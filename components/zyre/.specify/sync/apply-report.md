# Sync Apply Report

## Pass: 2026-10-14 (branch `fix/zyre-printf-format`, batch)

Applied: 2026-10-14T18:23:53Z
Based on: `.specify/sync/proposals.{json,md}` generated 2026-10-14T17:38:10Z at `85ae2a1f`
Approved by: Cornel Constantinescu, 2026-10-14 (all 9 proposals `approved: true`)
Mode: batch (no questions)

### Backup

Before editing, every touched file was copied to
`.specify/sync/backups/20261014T182353Z/`. That covers
`specs/001-zyre-bindings/{spec.md,data-model.md,research.md,contracts/izyre.md}`, plus the previous
`apply-report.md` and `drift-report.md`.

### Changes Made

#### Specs Updated

| Proposal | File | Requirement | Change Type |
|----------|------|-------------|-------------|
| P3 | `spec.md` (US2 narrative) | US2 | Modified: "iterator-based event streams… Send+Sync" → `recv()`/`try_recv()`, handle `Send` not `Sync` |
| P3 | `spec.md` (US2 AS3) | US2 AS3 | Modified: "iterates over incoming events" → "receives incoming events (`recv()` / `try_recv()`)" |
| P4 | `data-model.md` (ZyreNode) | FR-002 lifecycle | Modified: lifecycle line → `Created → Running → Draining → Done` |
| P4 | `data-model.md` (ZyreNode) | FR-002 drop | Modified: Drop = `stop()` (zyre_stop only if `Running`), then `zyre_destroy()` |
| P5 | `contracts/izyre.md` (create_node postconditions) | create_node | Modified: no discovery/messaging before `start()`; gossip endpoint is bound at creation |
| P5 | `contracts/izyre.md` (create_node errors) | create_node | Modified: `InvalidConfig` also covers a gossip endpoint that cannot be bound |
| P5 | `research.md` (R5) | create_node | Modified: an unbindable endpoint is detected after allocation; the node is destroyed before the error returns |
| P6 | `data-model.md` (NodeConfig table) | FR-005 headers | Modified: headers announced byte-for-byte |
| P6 | `data-model.md` (after Validation) | FR-005 / SC-005 | Added: "**Verbatim strings**" paragraph (constant `"%s"` format for printf-style setters) |
| P7 | `contracts/izyre.md` (Thread Safety) | create_node thread safety | Modified: creation serialised by a process-wide lock (CZMQ unlocked global init) |
| P8 | `data-model.md` (NodeConfig table) | FR-006 endpoint | Modified: `endpoint` only applied in gossip mode, ignored when `gossip` is `None` |
| P9 | `spec.md` (FR-010) | FR-010 | Modified: `ZYRE_BUILD_DIR` override, default `deps/zyre-build/`, DT_RPATH for libczmq/libzmq |
| — | `spec.md` (header) | metadata | Added: `**Last Modified**: 2026-10-14 (spec-sync apply P3-P9)` |

Every edit is the exact old→new text from `proposals.md`/`proposals.json`. Each old string matched
exactly once. P8 was applied as the BACKFILL. Its ALIGN alternative, rejecting `endpoint` without
`gossip`, was not chosen.

#### New Specs Created

None.

#### Supersede

None (no SUPERSEDE proposals).

#### Implementation Tasks Generated

There are 2 tasks in `.specify/sync/align-tasks.md`, one for P1 (FR-012, interior NUL panics) and one
for P2 (FR-005, timeouts above `i32::MAX`). Following the approval note *"approved as an ALIGN task
only: record in align-tasks.md, no code change in this PR"*, no code was changed. The
accompanying `data-model.md` validation text for P1 and P2 was **not** applied. It is recorded in
each task, to land together with the code fix, so the spec does not state a rule the code does not
yet enforce.

### Not Applied

| Proposal | Reason |
|----------|--------|
| — | Every proposal was approved and handled (7 applied, 2 turned into tasks). No unapproved proposal had an `approval_note`, so no open items were added. |

Still open, never proposed (from `proposals.json` `not_proposed`): L1 (`zyre_set_interface` may be
process-global; unverified), missing tests for `spec.md` edge case "nonexistent interface" and US1 AS4,
`tests/api_safety.rs` not asserting `!Sync`, and `src/repair_test_header_percent.rs` not gated by
`integration-tests`/`SERIAL_LOCK`.

No source code, interfaces crate or any file outside `components/zyre/{specs,.specify/sync}/` was
touched. No tests were run (spec text only; `deps/zyre-build/` is absent here).

### Next Steps

1. Review the spec diff: `git diff -- components/zyre/specs`.
2. Commit the spec and sync changes, and re-stamp the `spec_sync_*` frontmatter with the script.
3. Implement the P1 and P2 tasks in `align-tasks.md` in a follow-up PR.

---

## Pass: 2026-07-14

Applied: 2026-07-14
Based on: proposals from 2026-07-14 (P5 only)

## Scope

This pass applied **P5** (ZyreEvent::Stop reachability) with direction **B —
make STOP observable**, chosen after tracing the pinned zyre C source. P1–P4
(doc backfills for `plan.md`, `tasks.md` bodies, FR-008 wording, `research.md:84`)
were **not** applied in this pass and remain open.

## Why B (not the earlier "document-only" lean)

`zyre_stop()` sends `"STOP"` to the node's actor (`zyre.c:479`); the actor's
`zyre_node_stop()` enqueues a final `["STOP", own-uuid, own-name]` message on the
application inbox as its last act (`zyre_node.c:337-339`), then acks. So STOP is a
**graceful end-of-stream sentinel**, not an internal artifact — the C test at
`zyre.c:895-901` asserts a post-`stop()` `zyre_recv` returns it. The previous
binding set `started=false` in `stop()`, so `recv`/`try_recv` returned
`NotStarted` and the sentinel was unreachable — a latent bug that made the
contract's own `Stop => break` loop dead code. B is the faithful mapping.

## Changes Made

### Code changed

| File | Change |
|------|--------|
| `components/interfaces/src/izyre.rs` | Added `ZyreError::Stopped` variant + `Display` arm; reworded `RecvFailed` (interrupt, not stop); documented terminal `Stop` on `ZyreEvent::Stop` and on `IZyreNode::{stop,recv,try_recv}` |
| `components/zyre/src/node.rs` | Replaced `bool started` with `Cell<State> { Created, Running, Draining, Done }`; `start()` guards double-start; `stop()` → `Draining`; `recv()` drains and flips to `Done` on `Stop` (no further `zyre_recv` after `Done`); `try_recv()` returns `Ok(None)` when `Done`; added `ensure_running()` guard used by `join`/`leave`/`shout`/`whisper` |

### Specs updated

| File | Change |
|------|--------|
| `contracts/izyre.md` | Rewrote the consumer example to stop-then-drain-to-`Stop`; added a "Receive lifecycle" paragraph (NotStarted → drain → Stop → Stopped; single-threaded stop caveat) |
| `data-model.md` | `ZyreNode` field `started: bool` → `state: Cell<State>`; rewrote the state-transition diagram to `Created → Running → Draining → Done`; added the `Stop`-sentinel invariant |

### Tests added

| File | Test |
|------|------|
| `components/zyre/src/node.rs` | `recv_before_start_is_not_started` (unit) |
| `components/zyre/tests/integration.rs` | `stop_delivers_terminal_stop_event` — drains to `Stop`, then asserts `recv → Stopped` and `try_recv → Ok(None)` |

## Verification

- `cargo clippy -p zyre --tests` → **clean** (no warnings)
- `cargo test -p interfaces` → **28 passed, 0 failed** (incl. doctests)
- `cargo test -p zyre -- --test-threads 1`:
  - lib: **6 passed** (was 5; +`recv_before_start_is_not_started`)
  - `api_safety.rs`: **3 passed**
  - `integration.rs`: **4 passed** (was 3; +`stop_delivers_terminal_stop_event`; existing discovery/shout/whisper/gossip still pass)
  - doc: **1 passed**
- `cargo doc --no-deps -p interfaces -p zyre` → no zyre/izyre warnings (2 pre-existing warnings are in `igpu_services.rs`, unrelated)

## Not Applied (still open)

| Proposal | Reason |
|----------|--------|
| P1 (`plan.md` rewrite) | Awaiting apply — biggest remaining drift (CLAUDE.md points contributors here) |
| P2 (`tasks.md` task bodies) | The 2026-07-09 apply-report claimed this "rewritten", but T015/T027/T031-T033/T043/T046 still name `event.rs`/`builder.rs`/`error.rs`/`peer.rs` — only partially landed |
| P3 (FR-008 wording) | Awaiting apply; note FR-008 should also mention the STOP drain semantics now implemented |
| P4 (`research.md:84`) | Awaiting apply |
| SC-001 / SC-003 / SC-005 | Verification follow-ups (timed test / build timing / valgrind) |

## Follow-up: SC verification pass (2026-07-14)

Addressed the three verification follow-ups (previously "requires Linux + C deps"):

| SC | Result |
|----|--------|
| SC-001 (round-trip < 2 s) | **Verified.** Added `round_trip_within_two_seconds` (integration) asserting a real A→B→A exchange within 2 s. Discovery/whisper tests now resend-in-loop; added a `ZYRE_TEST_TIMEOUT_SCALE` env knob so they survive valgrind slowdown. |
| SC-003 (clean build < 5 min) | **Verified.** From-scratch `cargo build -p zyre` (incl. bindgen) = 2.27 s. The one-time C-deps build (`build_zyre.sh`) is separate and unchanged. |
| SC-005 (memory safety) | **Verified.** Miri can't cross the FFI boundary; added a valgrind harness (`run-valgrind.sh` + `valgrind.supp`). memcheck over lib + integration = 0 errors / 0 bytes lost attributable to the bindings; only C-library-internal reports are suppressed (incl. a benign self-overlapping `strcpy` inside czmq's `zsys_set_thread_name_prefix_str`). |

New/changed files: `components/zyre/tests/integration.rs` (SC-001 test + scaling + resend loops), `components/zyre/run-valgrind.sh`, `components/zyre/valgrind.supp`; tasks T035/T036/T055 checked, T042 annotated, T057 (SC-005) added.

Full CI gate re-run green: fmt clean, clippy `-D warnings` clean, tests (6 lib + 3 api_safety + 5 integration + 1 doc) pass, `cargo doc` warning-free.

## Next Steps

1. Review the diff and commit the SC-verification pass.
2. Remaining doc backfills P1–P4 were applied in commit `f6763fe`.
3. Optional: add a whisper-to-departed-UUID fire-and-forget test (`spec.md:81`) — not an SC gate.

---

## Pass: 2026-07-22 (AUTO-BACKFILL)

Applied: 2026-07-22
Based on: `.specify/sync/drift-report.{json,md}` generated 2026-07-22T22:33:44Z

### Scope

This component reported **0 drift** (17/17 FR+SC aligned) and **1 unspecced
feature**: `ZyreNode::stop()` (`src/node.rs:146-155`) is a silent no-op when
called outside the `Running` state (before `start()`, or a repeated call
while already `Draining`/`Done`) — a documentation gap in `data-model.md`'s
state-transition diagram, not a code or spec defect. No ALIGN/DEFECT items
were identified; `align-tasks.md` was not created.

### Backup

`specs/001-zyre-bindings/data-model.md` backed up to
`.specify/sync/backups/data-model.md.20260722162005.bak` before editing.

### Changes Made

| File | Change |
|------|--------|
| `specs/001-zyre-bindings/data-model.md` | Appended a note to the "ZyreNode Lifecycle" state-transition diagram documenting that `stop()` outside `[Running]` is a silent no-op/idempotent, not an error, and that only the `[Running] --stop()--> [Draining]` edge issues `zyre_stop()`. |

No source code was touched (source is read-only for this workflow).

### Counts

| Category | Count |
|---|---|
| BACKFILL applied | 1 (`data-model.md`) |
| SUPERSEDE/NEW_SPEC | 0 |
| ALIGN/DEFECT tasks appended | 0 (`align-tasks.md` not created — none qualified) |

### Deferred

None.

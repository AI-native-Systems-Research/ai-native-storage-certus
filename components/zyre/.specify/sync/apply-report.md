# Sync Apply Report

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

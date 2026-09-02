# Sync Apply Report

Applied: 2026-07-22
Mode: AUTO-BACKFILL
Source: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T22:40:55Z)

## Backups

Pre-edit copies of both spec files saved under `.specify/sync/backups/`:

- `001-spec.md.2026-07-22.bak`
- `002-spec.md.2026-07-22.bak`

(A stale `spec.md.2026-06-19.bak` from a prior sync run predates this pass and was left as-is.)

## Changes Made

### Specs Updated

| Spec | Item | Change Type | Resolution |
|------|------|--------------|------------|
| 001-remote-lookup-placeholder | Status header | Modified | `Synced (2026-06-19)` → `Superseded (2026-07-22)`; was mis-stamped — code implements 002, not this placeholder. |
| 001-remote-lookup-placeholder | New "Supersession Notice" section | Added | Banner pointing to `002-remote-lookup-rdma/spec.md`; documents that the `batch_lookup` `IpcHandle`→`u32` divergence (FR-001/SC-002 drift) is intentional per 002's Clarification Q1, not a defect. |
| 002-remote-lookup-rdma | Status header | Modified | `Draft` → `Synced (2026-07-22)`, noting the two backfills below. |
| 002-remote-lookup-rdma | FR-023 (receptacles) | Backfilled | Added the missing `responder_admin: IRemoteLookupRdmaResponderAdmin` receptacle to the list (code, `data-model.md`, and `contracts/iremote_lookup.md` already required it — only FR-023's prose omitted it). |
| 002-remote-lookup-rdma | FR-025 (responder wiring) | Backfilled | Named the `responder_admin` / `responder` receptacles explicitly, for consistency with the FR-023 fix. |
| 002-remote-lookup-rdma | FR-009 (client, failure) | Backfilled | Reclaim-timing prose rewritten to match `contracts/wire-protocol.md`'s ordering & safety invariants: a landing slot may be reclaimed on any RDMA_STATUS receipt (success or failure) while the peer is a live member; the FR-014 `DisconnectAck` gate applies only to the peer-departure path (no status received at all). Code already followed the contract; only the FR-009 text was out of date. |
| 002-remote-lookup-rdma | New FR-029 (out-of-interface lifecycle/test hooks) | Backfilled | Documents `RemoteLookupComponent::peers_seen()`, `::signal_shutdown()`, `::shutdown()` as intentional, non-`IRemoteLookup` `pub fn`s needed for multi-actor zyre/czmq teardown ordering and test discovery; not part of the interface contract. |

### New Specs Created

None (`NEW_SPEC: none` per instructions).

### Align/Defect/Ambiguous Tasks Generated

Appended to `.specify/sync/align-tasks.md`:

- **Task 1** (medium): missing `tests/mesh.rs` coverage for User Story 7 / `tasks.md` T025
  (peer-Exit: cached-reply drop, in-progress→unsatisfied transition, `DisconnectAck`-gated slot
  reclaim). Implementation reviewed as correct; test-only gap, out of scope for this Markdown-only
  pass.
- **Task 2** (low, monitoring only): watch that `peers_seen()`/`signal_shutdown()`/`shutdown()`
  stay test/teardown-only; promote to a real interface/admin receptacle if a production caller
  outside teardown ordering appears.

### Deferred Items

None. Every drift item in the report had an explicit resolution directive (SUPERSEDE 001, BACKFILL
FR-023/FR-009/three methods on 002, ALIGN-task the US7 test gap) and was applied; nothing was
ambiguous enough to require deferral.

## Not Applied

None — no proposal was rejected; all drift items had an explicit, unambiguous resolution.

## Files Touched

- `specs/001-remote-lookup-placeholder/spec.md` (superseded banner + status)
- `specs/002-remote-lookup-rdma/spec.md` (FR-023, FR-025, FR-009 backfills; new FR-029; status)
- `.specify/sync/align-tasks.md` (created)
- `.specify/sync/apply-report.md` (this file, overwrites the 2026-06-19 report from a prior sync
  run against 001 only)
- `.specify/sync/backups/001-spec.md.2026-07-22.bak`, `002-spec.md.2026-07-22.bak` (created)

No source code under `src/` or any other non-Markdown file was modified, per the hard rule
restricting this pass to `specs/**` and `.specify/sync/**` Markdown.

## Next Steps

1. Review the backfilled FR-009/FR-023/FR-029 text in `specs/002-remote-lookup-rdma/spec.md` against
   the actual code (`src/lib.rs`, `src/actor.rs`) to confirm wording precision.
2. Assign Task 1 in `align-tasks.md` (T025 mesh test) to an implementation pass.
3. Commit: `git add specs/ .specify/sync/ && git commit -m "sync: apply drift resolutions (auto-backfill) for remote-lookup"`

---

# 2026-08-07 Sweep

Applied: 2026-08-07 on branch `sync/spec-drift-sweep-20260807`
Mode: Interactive drift sweep (`component-sync-specs`, all specced components)
Source: `.specify/sync/drift-report.{json,md}` (regenerated 2026-08-07).

Drift headline: code matches spec 002 almost exactly (38 aligned, 2 low/med
drift). Spec 001 is correctly superseded — no code action. Five load-bearing
behaviors were unspecced.

## Changes Made (branch only, nothing committed to `unstable`)

### Specs Updated (BACKFILL — document reality; applied)

| Spec | Requirement | Change |
|------|-------------|--------|
| 002-remote-lookup-rdma | Header | `Last Synced 2026-08-07` re-sweep note |
| 002-remote-lookup-rdma | FR-030 (new) | `caller_wait` caller/op-deadline decoupling (background-continue) |
| 002-remote-lookup-rdma | FR-031 (new) | `connection_teardown_timeout` / `tick_orphans` force-reclaim backstop |
| 002-remote-lookup-rdma | FR-032 (new) | orphan-reuse guard (memory-safety) |
| 002-remote-lookup-rdma | FR-033 (new) | extra `LookupConfig` fields: `actor_cpu`, `discovery`, `node_endpoint` |
| 002-remote-lookup-rdma | FR-034 (new) | `integrity-check` Cargo feature (build plumbing) |

### Code (doc-only correction — applied on branch)

| File | Change | Category |
|------|--------|----------|
| `src/lib.rs:1-8` | Module header docstring: removed stale "protocol unbuilt / all-NotFound" prose; describes shipped KEY_QUERY→RDMA behavior | Doc correction (FR-001 doc-drift, Medium) |
| `src/lib.rs:249-253` | `batch_lookup` docstring: same correction; clarifies `Ok(())` semantics and the uninitialized-no-actor `NotFound` path (which the existing doctest exercises) | Doc correction |

Verified: `cargo build -p remote-lookup` clean (doctest unaffected).

### Implementation Tasks (see align-tasks.md 2026-08-07 section)

- Task 3 — FR-018 log unknown wire frames (Low): **queued, not drafted** (per
  sweep pacing: non-HIGH ALIGN items are queued, not drafted).
- Task 4 — FR-001 stale docstrings (Medium): **applied this pass** (doc-only),
  logged for the audit trail.

### Not Applied / Deferred

None deferred. Spec 001 divergences remain the intentional, documented
supersession (no code action). The one remaining code item (FR-018 log line)
is queued as a low-severity align-task.

## Next Steps

1. Implement Task 3 (FR-018 unknown-frame log) at the maintainer's discretion.
2. Review the FR-030…FR-034 backfills for wording precision against the code.
3. Commit on the branch only — never to `unstable`.

---

# 2026-08-20 Sweep (Phase B)

Applied: 2026-08-20
Mode: Phase B shared-policy resolution (`.specify/sync/PHASE_B_POLICY.md`, "all other components"
default — no special per-component note).
Source: `.specify/sync/drift-report.{json,md}` (current pending report: 6 drifted, 3 unspecced).

Drift headline: 5 of the 6 drift items are on the **superseded** spec 001 (intentional divergence
from the placeholder → BACKFILL as inline superseded-by-002 annotations); the 1 remaining drift is
spec 002 FR-018 logging (spec correct, code silent → ALIGN task). All 3 unspecced behaviors were
BACKFILLED into spec 002. No `.rs` source modified; no cargo run.

## Backups

Pre-edit copies saved under `.specify/sync/backups/` preserving the component-relative path
(per policy `backups/<same-relative-path>.bak`):

- `backups/specs/001-remote-lookup-placeholder/spec.md.bak`
- `backups/specs/002-remote-lookup-rdma/spec.md.bak`

(Earlier flat-named backups `001-spec.md.2026-07-22.bak`, `002-spec.md.2026-07-22.bak`,
`spec.md.2026-06-19.bak` from prior passes were left as-is.)

## Specs Updated

| Spec | Requirement | Change Type | Resolution |
|------|-------------|-------------|------------|
| 001-remote-lookup-placeholder | Status header | Modified | Added `re-swept 2026-08-20` note explaining the per-requirement supersession annotations below. |
| 001-remote-lookup-placeholder | FR-001 | BACKFILL (annotate) | Inline *Superseded by 002 FR-001*: shipped `&[(CacheKey, u32)]`, `IpcHandle` dropped, `Ok(())` ⇒ resident. |
| 001-remote-lookup-placeholder | FR-003 | BACKFILL (annotate) | Inline *Superseded by 002*: real KEY_QUERY→RDMA protocol, no per-entry placeholder log. |
| 001-remote-lookup-placeholder | FR-004 | BACKFILL (annotate) | Inline *Superseded by 002 FR-005..FR-012*: real zyre + RDMA I/O; `Ok(())` when resident, `NotFound` only on deadline. |
| 001-remote-lookup-placeholder | FR-008 | BACKFILL (annotate) | Inline *Superseded by 002 FR-029*: intentional out-of-interface hooks `peers_seen`/`signal_shutdown`/`shutdown`. |
| 001-remote-lookup-placeholder | SC-002 | BACKFILL (annotate) | Inline *Superseded by 002 FR-001*: compiles with `(CacheKey, u32)`; type-equality goal removed. |
| 002-remote-lookup-rdma | Status header | Modified | Added 2026-08-20 note listing the three FR backfills and the widened FR-018 ALIGN task. |
| 002-remote-lookup-rdma | FR-006 | BACKFILL-UNSPECCED | Added `AlreadyExists` size-collision guard: success only at matching size; differing size discards the private slot and never evicts the resident entry (`src/actor.rs:576-591`). |
| 002-remote-lookup-rdma | FR-014 | BACKFILL-UNSPECCED | Documented the fixed 500 ms `DISCONNECT_ACK_TIMEOUT` ack-handshake bound (`src/actor.rs:37`), deliberately not a `LookupConfig` knob; distinct from FR-031's `connection_teardown_timeout`. |
| 002-remote-lookup-rdma | FR-018 | BACKFILL-UNSPECCED | Named the malformed/truncated-frame ignore class (b) alongside unknown `msg_type` (a); both MUST be logged. Logging code work → ALIGN task. |
| 002-remote-lookup-rdma | Edge Cases | Added | Two scenarios: unknown/malformed frame logged+dropped (FR-018); `AlreadyExists` publish race size check (FR-006). |

## Align Tasks Generated

Appended to `.specify/sync/align-tasks.md` (2026-08-20 section):

- **Align 002/FR-018** (Low): log both the unknown-`msg_type` arm (`src/actor.rs:330`) and the
  malformed-decode arm (`src/actor.rs:314`) before dropping. Supersedes the narrower 2026-08-07
  Task 3 (unknown arm only). Source/test work — out of scope for this Markdown-only pass.

## Unspecced Backfilled

| Feature | Location | Backfilled into |
|---------|----------|-----------------|
| DISCONNECT_ACK_TIMEOUT (fixed 500 ms DisconnectAck wait) | `src/actor.rs:37,1020-1033` | FR-014 |
| Malformed/truncated wire frame ignore | `src/actor.rs:314` | FR-018 (logging folded into ALIGN task) |
| publish_success AlreadyExists size-collision guard | `src/actor.rs:576-591` | FR-006 |

## Resolved

None (no drift item was pre-fixed on the main thread for this component).

## Human Decision

None (all items were unambiguous after reading the cited code).

## Files Touched

- `specs/001-remote-lookup-placeholder/spec.md` (status + 5 requirement annotations)
- `specs/002-remote-lookup-rdma/spec.md` (status + FR-006/FR-014/FR-018 + 2 edge-case scenarios)
- `.specify/sync/proposals.md`, `.specify/sync/proposals.json` (rewritten for this pass)
- `.specify/sync/align-tasks.md` (2026-08-20 section appended)
- `.specify/sync/apply-report.md` (this section), `.specify/sync/apply-report.json`
- `.specify/sync/backups/specs/001-remote-lookup-placeholder/spec.md.bak`,
  `.specify/sync/backups/specs/002-remote-lookup-rdma/spec.md.bak` (created)

No `.rs` source or any non-Markdown file was modified; cargo was not run.

---

# 2026-09-02 Sweep (re-verification + stamp)

Applied: 2026-09-02T21:39:18Z
Mode: Re-verification pass — re-checked every FR/SC against source at commit `2fc1cd3c`, then
stamped the drift report (it was left `Generated: pending` after the 2026-08-20 sweep).
Source: `.specify/sync/drift-report.{json,md}` (2026-08-20 findings, re-verified).

## Verification result

Re-read the full component source (`src/actor.rs`, `src/lib.rs`, `src/server.rs`, `src/wire.rs`)
and `components/interfaces/src/iremote_lookup.rs`, plus `tests/mesh.rs` and `Cargo.toml`. Every
2026-08-20 finding still holds against the current code:

- **FR-018 (Low, ALIGN — still open)**: `on_wire` drops both frame classes silently —
  `Err(_) => return` (`src/actor.rs:314`) and `WireMessage::Unknown { .. } => {}` (`src/actor.rs:330`).
  No `logger` call on either arm. The *ignore* half is aligned; the *logging* half is still a code
  gap (align-tasks.md 2026-08-20 task). Confirmed unchanged.
- **FR-006 size-collision guard** (`src/actor.rs:576-591`), **FR-014 `DISCONNECT_ACK_TIMEOUT`**
  (`src/actor.rs:37,1020-1033`), **FR-030 caller_wait**, **FR-031 connection_teardown_timeout /
  tick_orphans** (`src/actor.rs:849-885`), **FR-032 orphan-reuse guard** (`:441-443,630-632,772-774`),
  **FR-033 LookupConfig actor_cpu/discovery/node_endpoint** (`iremote_lookup.rs:65-78`), **FR-034
  integrity-check** (`Cargo.toml:14`): all present and matching the backfilled spec text.
- **FR-001 lib.rs docstrings**: the 2026-08-07 doc correction is in place (`src/lib.rs:1-10,251-257`
  describe the shipped KEY_QUERY→RDMA protocol and `Ok(())` ⇒ resident semantics). No doc-drift.
- **Spec 001** superseded-by-002 annotations (FR-001/003/004/008, SC-002) present and correct.
- **US7 test-coverage gap** (align-tasks.md Task 1 / tasks.md T025): still open — `tests/mesh.rs`
  has `slot_survives_timeout_...` and `stuck_orphan_is_force_reclaimed_...` but no dedicated
  zyre-`Exit` scenario exercising `on_exit`/`teardown_peer`. Not drift (feature is implemented).

## Changes this pass

- Stamped `drift-report.md` frontmatter (`spec_sync_*`) and set `Generated` timestamp; set
  `drift-report.json` `generated` + `spec_sync_*` fields. `spec_sync_inputs_sha256` =
  `44b753dd84f804f5678b32c34f6c26da2595b73203fc4cb56d8097f6b4302c5f`, `spec_sync_git_commit` =
  `2fc1cd3c`, `spec_sync_drift_status` = `drift` (one open actionable Low ALIGN: FR-018 logging).
- No spec `.md` edits (all 2026-08-20 backfills already applied — no spec backup needed).
- No new ALIGN task (FR-018 logging task already recorded); no HUMAN_DECISION.
- No `.rs` source modified; cargo not run.

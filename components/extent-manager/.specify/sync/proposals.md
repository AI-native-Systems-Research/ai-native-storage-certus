# Sync Proposals — extent-manager (Phase B)

**Generated**: 2026-08-20
**Based on**: `drift-report.json` (extent-manager, 2 drifted, 0 not_implemented, 0 unspecced)

Both drift items classify as **BACKFILL** (spec-lag: code/reality is correct, spec text
is stale). No ALIGN, no UNSPECCED, no RESOLVED, no HUMAN_DECISION.

| ID | Requirement | Direction | Severity | Verdict |
|----|-------------|-----------|----------|---------|
| P1 | FR-030 | BACKFILL | moderate | `volatile_write_cache` fix has landed; remove stale "does not compile" status |
| P2 | plan.md-layout-refs | BACKFILL | minor | Drop non-existent `block_device` receptacle + `v2/` path from plan.md |

---

## P1 — FR-030 (`volatile_write_cache`)  →  BACKFILL

**Spec (before)**: FR-030's parenthetical and the top Sync note stated *"Implementation
status 2026-08-07: this feature does not yet compile"* — `BlockDeviceClient::flush()`
missing, `Command::FlushSync`/`Completion::FlushDone` absent from interfaces, a fix
"drafted on the branch", an align-task queued, and "add a CI job".

**Code (actual)**: The cross-crate fix has **landed** on the current tree:
- `BlockDeviceClient::flush()` exists, gated on `volatile_write_cache`, sending
  `Command::FlushSync` and awaiting `Completion::FlushDone` (`block_io.rs:168`).
- The interfaces crate defines `Command::FlushSync` (`iblock_device.rs:411`) and
  `Completion::FlushDone` (`iblock_device.rs:501`).
- The checkpoint path calls `metadata_client.flush()` under the feature gate
  (`lib.rs:308-310`).

**Rationale**: Spec-lag — the code now satisfies the intended FR-030 semantics; the
only drift is the stale "does not compile / drafted / queued" status. BACKFILL the
FR-030 parenthetical and the top Sync note to state the feature is implemented and
building.

**Before**
> (Implementation status 2026-08-07: this feature does not yet compile — see the Sync
> note at the top of this spec and `.specify/sync/align-tasks.md`.)

**After**
> (Implementation status 2026-08-20: implemented and building. `BlockDeviceClient::flush()`
> (`block_io.rs:168`) sends `Command::FlushSync` and awaits `Completion::FlushDone` — both
> defined in the interfaces crate (`iblock_device.rs:411,501`) — and is called from the
> checkpoint path (`lib.rs:308-310`). Enable with `--features volatile_write_cache`.)

---

## P2 — plan.md layout references  →  BACKFILL

**Spec (before)**: `plan.md` describes a `block_device` data-device `IBlockDevice`
receptacle (Technical Context "Storage" block, line 25; Component Structure receptacle
list, line 39) and roots the source tree at `components/extent-manager/v2/` (line 213).

**Code (actual)**: The shipped component exposes only `metadata_device` + `logger`
receptacles (spec FR-001) over a flat `src/` — there is no `v2/` path. The data path
is owned by the caller (`dispatcher`/`dispatcher-p2p`, FR-036); the component performs
no data-device I/O.

**Rationale**: Spec-lag in the planning doc — code and `spec.md` already agree on the
`metadata_device` + `logger` receptacle model and flat `src/`. BACKFILL the plan to
reality.

**Before**: two receptacles incl. `block_device: IBlockDevice (data device)`; tree
rooted at `components/extent-manager/v2/`.

**After**: single `metadata_device` receptacle (plus `logger`), with a note that the
data device is caller-owned per FR-036; tree rooted at `components/extent-manager/`; a
Last-Synced 2026-08-20 note added to the plan header.

# Align Tasks: certus-server

Generated: 2026-07-22
Based on: drift-report.md / drift-report.json (2026-07-22), apply-report.md (2026-07-22)

Items below were identified during the AUTO-BACKFILL sync-apply pass but were
**not** resolved automatically because they require a human decision (either
because the correct spec-vs-code direction is ambiguous, or because the fix
touches non-spec files this pass is not permitted to edit). Each task lists a
severity, the affected spec/location, and a recommended next step.

---

## Task 1 — Stale README.md describes removed extent-manager/metadata-device architecture

**Severity**: Medium (doc)
**Type**: ALIGN (documentation)
**Location**: `apps/certus-server/README.md:9-72,100-108`

**Finding**: `README.md`'s "Component Stack" section and architecture diagram still
describe a metadata NVMe device plus an `IExtentManager` persistence layer
backing the dispatch map. This directly contradicts spec 002's FR-008b/FR-008c
clarification (Session 2026-05-22) that there is *no* metadata device and *no*
extent-manager-backed persistence — the dispatch map is ephemeral. The README's
gRPC API table also lists only 4 of the 15 defined RPCs (missing `Touch`,
`Reserve`, `CopyToStore`, `CommitStore`, `AbortStore`, `ClearMemoryTier`,
`FlushToSsd`, `Pin`, `Unpin`, `TakeEvents`, `GetIoStats`), and its CLI Options
table omits `--drive-count`, `--format`, `--memory-tier-size`,
`--poller-base-cpu`, `--max-eviction-attempts`,
`--memory-tier-eviction-threshold`, `--otel-endpoint`, `--otel-service-name`.

**Why not fixed here**: `README.md` lives under `apps/certus-server/` but is
**not** under `apps/certus-server/specs/**`; this sync-apply pass is restricted
to editing Markdown under `specs/**` and `.specify/sync/**` only, so the README
was intentionally left untouched.

**Recommended next step**: Rewrite `README.md`'s Component Stack section,
architecture diagram, gRPC API table, and CLI Options table to match the
current no-extent-manager, 15-RPC (16 counting `GetIoStats` under
`rw-telemetry`), 13-flag implementation described in specs 001-003.

---

## Task 2 — FR-008 drift: EvictionPolicyLru and RemoteLookup absent from spec 001's Component Stack

**Severity**: Low
**Type**: DEFECT / ALIGN (spec under-documents code)
**Location**: `apps/certus-server/src/main.rs:187-196,263-270`; `apps/certus-server/specs/001-grpc-dispatcher-server/spec.md` FR-008 and "Component Stack" section

**Finding**: The server's actual init order also constructs and binds an
`EvictionPolicyLru` component (shared by dispatch-map and memory-tier) and a
`RemoteLookup` component (bound to the dispatcher). Neither appears in FR-008's
stack description or the spec's numbered "Component Stack" list (currently:
SPDK env, Logger, GPU Services, Dispatch Map, Memory Tier, Dispatcher).

**Why deferred**: This pass's explicit backfill scope was limited to FR-010,
FR-020, `GetIoStats`/`rw-telemetry`, and `--memory-tier-eviction-threshold`.
Extending the Component Stack list is straightforward but was left for a
follow-up backfill pass to keep this change set narrowly scoped and reviewable.

**Recommended next step**: BACKFILL — add `EvictionPolicyLru` and
`RemoteLookup` as steps in spec 001's "Component Stack" list and reference them
from FR-008.

---

## Task 3 — FR-011 drift: `IpcHandle.offset` field undocumented

**Severity**: Low
**Type**: DEFECT / ALIGN (spec under-documents code)
**Location**: `apps/certus-server/proto/dispatcher.proto:59-70`; `apps/certus-server/src/service.rs:275,375,651`; `apps/certus-server/specs/001-grpc-dispatcher-server/spec.md` FR-011

**Finding**: `IpcHandle` has a 4th field, `offset` (`uint64`), actively used to
address a sub-block within one shared CUDA allocation (`dev_ptr + offset`).
FR-011 documents only 3 fields (`cuda_ipc_handle`, `size`, `gpu_device_id`).

**Why deferred**: Same reasoning as Task 2 — out of this pass's explicit
backfill scope; recommend folding in on the next pass alongside Task 2.

**Recommended next step**: BACKFILL — add `offset: uint64` to FR-011's field
list and the `IpcHandle` Key Entity description, documenting its sub-block
addressing purpose.

---

## Task 4 — Unused `ERROR_CODE_DUPLICATE_KEY` proto enum value

**Severity**: Low
**Type**: AMBIGUOUS (needs a code-vs-spec decision)
**Location**: `apps/certus-server/proto/dispatcher.proto:77`; `apps/certus-server/src/service.rs:151-162`

**Finding**: `ErrorCode.ERROR_CODE_DUPLICATE_KEY` is defined in the proto but
never returned; duplicate-key rejection (FR-015) actually uses a tonic
`Status::invalid_argument` with a free-text message, not a structured
`EntryResult.error_code`.

**Why deferred**: Two equally valid resolutions exist and only a maintainer can
choose: (a) wire `ERROR_CODE_DUPLICATE_KEY` into the duplicate-key rejection
path (code change, out of scope for a spec-only pass and touches `.rs`/`.proto`
files this pass cannot edit), or (b) remove the unused enum value from the
proto and document the actual `Status::invalid_argument` behavior in FR-015
(also a `.proto` edit, out of scope here). Neither can be done by editing
`specs/**` Markdown alone.

**Recommended next step**: Maintainer decides (a) or (b); file a follow-up
issue/PR since it requires a `.proto`/`.rs` change, not a spec-only fix.

---

## Summary

| # | Severity | Spec/Location | Type | Status |
|---|----------|----------------|------|--------|
| 1 | Medium | README.md | ALIGN (doc) | Deferred — out of edit scope (not under specs/) |
| 2 | Low | 001-grpc-dispatcher-server / FR-008 | DEFECT/ALIGN | Deferred — out of this pass's backfill scope |
| 3 | Low | 001-grpc-dispatcher-server / FR-011 | DEFECT/ALIGN | Deferred — out of this pass's backfill scope |
| 4 | Low | dispatcher.proto ERROR_CODE_DUPLICATE_KEY | AMBIGUOUS | Deferred — requires code/proto change + maintainer decision |

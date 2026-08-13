# Spec-Sync Align Tasks — gpu-services

Generated: 2026-07-22
Based on: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22)
Mode: AUTO-BACKFILL apply pass. Items below were judged as genuine judgment
calls, ambiguous intent, or documentation-only drift, and were therefore
**deferred** rather than auto-resolved. They require a human decision.

---

## Task: Align 001-gpu-cuda-services/FR-008 (inter-spec conflict with 002-gpu-ssd-dma-prepare/FR-021,FR-022,FR-023)

**Severity**: Medium

**Spec Requirement**: Spec 001 FR-008 states: "Component MUST expose all
functionality exclusively through the `IGpuServices` interface defined in
`components/interfaces`."

**Current Code**: `src/dma.rs` (lines ~352-702) exposes
`create_spdk_dma_buffer_from_gpu_bar`, `create_spdk_dma_buffer_from_phys`,
`create_spdk_dma_buffer_from_bar_direct`, and `get_phys_addr` as public
free functions gated behind the `p2p` feature — not as `IGpuServices`
methods. They are called directly by `src/bin/p2p_server.rs` and
`tests/gpu_nvme_p2p.rs`, bypassing the interface entirely. This is required
by spec 002 FR-021/FR-022/FR-023, which explicitly mandate these three
constructors be standalone functions (not interface methods), so the
implementation is faithful to spec 002 but in direct violation of spec 001
FR-008 as literally written.

**Required Change**: This is a genuine judgment call between two specs
that disagree, not a code defect — DEFERRED to human review rather than
auto-resolved. Recommended direction (for human sign-off): relax spec 001
FR-008 to add an explicit carve-out for the `p2p`-gated low-level
GDRCopy/VFIO DMA-buffer constructors (e.g. "...exclusively through the
`IGpuServices` interface, with the exception of the `p2p`-gated low-level
DMA-buffer constructors specified in spec 002 FR-021/FR-022/FR-023, which
are intentionally standalone functions because they operate on raw
device/BAR pointers that are awkward to expose through a dyn-safe trait
method"). Alternative (not recommended without further discussion): move
those three functions onto `IGpuServices` and break spec 002's explicit
"standalone function" requirement instead. A human must choose the
direction; this task should not be auto-applied.

**Files to Modify** (pending human decision):
- `components/gpu-services/specs/001-gpu-cuda-services/spec.md` (FR-008)
- `components/gpu-services/specs/002-gpu-ssd-dma-prepare/spec.md` (FR-021/FR-022/FR-023, if the alternative direction is chosen)

---

## Task: Align 001-gpu-cuda-services/FR-005 (unpin_memory never performs full CUDA unregistration for "locally-pinned" memory)

**Severity**: Low

**Spec Requirement**: FR-005 states, in part: "For IPC-imported memory,
`unpin_memory` removes internal tracking only ... For locally-pinned
memory, full CUDA unregistration is performed."

**Current Code**: `unpin_memory` (`src/lib.rs:227-245`) only ever removes
the pointer from the internal `pinned` `HashSet<usize>`; it never calls
`cudaHostUnregister` or any other CUDA unregistration API. `pin_memory` /
`unpin_memory` operate exclusively on `GpuIpcHandle` values, which by
construction (`src/ipc.rs:70`, `GpuIpcHandle::new`) are always
IPC-derived — there is no code path that produces a "locally-pinned"
(i.e., non-IPC) pointer today. The "locally-pinned memory / full CUDA
unregistration" branch the spec describes therefore has no reachable
implementation.

**Required Change**: Ambiguous whether this was (a) aspirational spec text
written ahead of a "local pinning" feature that was never built, or (b) an
intentional simplification once the team realized all pinning in this
component is IPC-derived. Could not confidently classify as either
BACKFILL (implement the missing branch) or a pure documentation trim
without product input — DEFERRED per hard-rule "if intentional backfill,
else defer." Recommended options for human decision:
1. Trim the "locally-pinned memory" clause from FR-005 since no such
   pointer can currently reach `pin_memory`/`unpin_memory` (documentation
   fix, no code change), or
2. Implement a genuine local-pin path (e.g. a `pin_local_memory(ptr, size)`
   that calls `cudaHostRegister`, tracked separately from IPC-derived
   pointers) so `unpin_memory` can distinguish and fully unregister it
   (code change).

**Files to Modify** (pending human decision):
- `components/gpu-services/specs/001-gpu-cuda-services/spec.md` (FR-005), and/or
- `components/gpu-services/src/lib.rs` (`pin_memory`/`unpin_memory`, ~lines 170-245) — **not modified by this pass; source code is out of scope for spec-sync-apply.**

---

## Task: Align 001-gpu-cuda-services/FR-015 (`register_host_memory` treats SPDK EBUSY (-16) as success)

**Severity**: Low

**Spec Requirement**: FR-015 states: "If `cudaHostRegister` succeeds but
`spdk_mem_register` fails, the method MUST roll back by calling
`cudaHostUnregister` before returning the error."

**Current Code**: `register_host_memory` (`src/lib.rs:937-985`, rollback
special-case at `src/lib.rs:973-981`) correctly rolls back on genuine
`spdk_mem_register` failures, but additionally treats return code `-16`
(`EBUSY`, "already registered with SPDK") as a success case and skips the
rollback path entirely in that situation. This special-casing is not
mentioned anywhere in FR-015 or its Assumptions.

**Required Change**: Ambiguous whether the EBUSY special-case is an
intentional idempotency accommodation (e.g. to tolerate double-registration
from a caller retry) or an oversight that silently masks a real
double-registration bug — DEFERRED per hard-rule "if intentional backfill,
else defer." Recommended options for human decision:
1. If intentional: backfill FR-015's Assumptions with an explicit clause
   documenting that `spdk_mem_register` rc == -16 (EBUSY, "already
   registered") is treated as success for idempotency, matching
   `pin_memory`'s idempotent semantics (FR-005).
2. If unintentional: remove the special-casing in code so any
   `spdk_mem_register` failure (including EBUSY) triggers the documented
   rollback — this is a code change and out of scope for this
   documentation-only spec-sync-apply pass.

**Files to Modify** (pending human decision):
- `components/gpu-services/specs/001-gpu-cuda-services/spec.md` (FR-015 Assumptions), and/or
- `components/gpu-services/src/lib.rs` (`register_host_memory`, ~lines 937-985) — **not modified by this pass; source code is out of scope for spec-sync-apply.**

---

## Task: Refresh stale `CLAUDE.md` skeleton description (documentation drift, not spec drift)

**Severity**: Low (informational / doc-only)

**Spec Requirement**: N/A — this is not a spec-vs-code drift; it is a
doc-vs-code drift flagged by the drift report.

**Current Code**: `components/gpu-services/CLAUDE.md` still describes the
component as "currently a skeleton — `initialize()` and `shutdown()`
lifecycle methods with an optional `ILogger` receptacle." The component now
implements 25 functional requirements across spec 001 (22 base + FR-023,
FR-024, FR-025 backfilled in this pass) and 24 in spec 002 (46 FRs total
across ~3,700 lines): CUDA device discovery, IPC handle handling,
pin/verify tracking, sync/async DMA, CUDA streams, SPDK registration, and
GDRCopy P2P — plus the unspecced `gpu-p2p-server` binary now covered by new
spec 003.

**Required Change**: Refresh `components/gpu-services/CLAUDE.md`'s
Overview section to describe the current architecture (CUDA FFI, device
discovery, IPC, DMA, streams, SPDK/GDRCopy P2P, and the `gpu-p2p-server`
binary) instead of the original skeleton description. **This file is
outside `components/gpu-services/specs/**` and `.specify/sync/**`, so per
this pass's hard rules it was NOT edited here.** Logged as a follow-up doc
task instead.

**Files to Modify** (follow-up, out of scope for this pass):
- `components/gpu-services/CLAUDE.md` (Overview section)

---

# 2026-08-07 Sweep — Resolutions & Remaining Open Items

Branch: `sync/spec-drift-sweep-20260807`. Status update on the 2026-07-22
align-tasks after the sweep re-analysis and maintainer fork decisions.

## RESOLVED this sweep (spec-only BACKFILL applied)

- **FR-008 vs 002/003 conflict (Medium)** — ✅ RESOLVED. Maintainer chose
  "Soften FR-008 wording (backfill)". FR-008 now carries an explicit documented
  carve-out for the `p2p`-gated DMA-buffer builders. See apply-report.md
  2026-08-07 sweep section.
- **FR-005 locally-pinned unregistration (Low)** — ✅ RESOLVED via option 1
  (documentation trim). FR-005 reworded to state `unpin_memory` is
  tracking-removal-only in all cases; host un/registration lives in
  FR-015/FR-016. No code change.

## STILL OPEN (not addressed this sweep)

- **FR-015 EBUSY special-case (Low)** — ⏳ OPEN. `register_host_memory` treats
  SPDK `EBUSY` (-16) as success and skips rollback. Not re-surfaced as drift
  this sweep. Still needs a maintainer decision: document the idempotency
  accommodation in FR-015 Assumptions, or remove the special-case in code.
- **CLAUDE.md skeleton staleness (Low, doc-only)** — ⏳ OPEN. Component-root
  `CLAUDE.md` Overview still describes a bare `initialize()`/`shutdown()`
  skeleton; outside `specs/**`, deliberately not edited by spec-sync.

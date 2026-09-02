# Drift Resolution Proposals — spdk-env (Spec-Sync)

Generated: 2026-09-02
Based on: `components/spdk-env/.specify/sync/drift-report.json` (specs analyzed: 2;
requirements checked: 26; aligned: 24; drifted: 1; not_implemented: 1; unspecced: 0)

Source (`src/*.rs`) unchanged since 2026-06-11; independently re-verified this run.

## Summary

| Resolution Type | Count |
|-----------------|-------|
| BACKFILL (spec → code) | 0 |
| ALIGN (task, no code change) | 0 (Task 1 carried forward) |
| BACKFILL-UNSPECCED | 0 |
| RESOLVED (already fixed) | 1 (stale-crate-paths) |
| HUMAN_DECISION | 1 (SC-001 device-type scope) |
| not_implemented handled (LEAVE + NOTE) | 1 (FR-015) |

## Proposals

### Proposal 1 — 002-spdk-env-vfio-init / SC-001 device-type scope (also US1 narrative + Clarifications)

**Direction**: HUMAN_DECISION (do NOT auto-backfill)

**Rationale**: SC-001 ("discovers 100% of available VFIO-bound devices ...
matching /sys/bus/pci/drivers/vfio-pci"), the User Story 1 narrative ("all
SPDK-supported device types (NVMe, virtio-blk, etc.)"), and the recorded
Clarifications answer (Session 2026-04-07, "All SPDK-supported device types
bound to VFIO") all promise broader device discovery than the implementation
delivers: `enumerate_devices` (`src/env.rs:164-181`) enumerates only the NVMe
PCI driver. FR-006 already scopes enumeration to NVMe-only and the code matches
FR-006, so this is both a spec-vs-code gap and an internal spec contradiction.

Two valid resolutions, each changing what is *promised*:
- (a) **Extend the code** to enumerate additional SPDK PCI drivers (virtio-blk,
  etc.) — a functional change, filed as a code task; or
- (b) **Narrow the spec** (SC-001 + US1 narrative + Clarifications) to NVMe-only
  to match FR-006 + code — a text backfill.

Because option (b) would overwrite a *recorded product/clarification decision*
to match a partial implementation, this is not a confident text-only backfill
and is deferred to a human. Tracked in `align-tasks.md` Task 1 (carried forward
from a prior run; re-confirmed as the live drift this run).

**Before / After**: none (no spec text changed this run).

**approved**: false

---

### Proposal 2 — 002-spdk-env-vfio-init / FR-015 (not_implemented)

**Direction**: LEAVE + NOTE (not_implemented; not obsolete, not a code bug)

**Rationale**: FR-015 ("skip devices in use by another process, log a warning
per skipped device, return only successfully probed devices") originates from a
recorded Clarifications decision and is genuinely intended future behavior. The
spec text already self-flags it: "(Future: not yet implemented. Currently all
matching devices are claimed; user must ensure exclusive access via system
configuration.)". `enumerate_devices` uses a non-attach callback (returns
non-zero, per FR-006), so devices are never claimed during enumeration and there
is no probe-and-skip step to gate. Per policy this is "genuinely needed but
missing → leave and note, don't invent" — no spec rewrite, no invented
implementation, no ALIGN code task. Residual present-tense wording in User Story
1 Acceptance Scenario 4 / its Edge Case remains tracked as `align-tasks.md`
Task 4.

**Before / After**: none.

**approved**: false

---

### Proposal 3 — 002-spdk-env-vfio-init / stale-crate-paths (RESOLVED)

**Direction**: RESOLVED (backfilled 2026-08-20; verified present this run)

**Rationale**: `spdk-sys` moved to `lib/spdk-sys/` and `component-framework` to
`lib/component-framework/`; supporting docs cited old `components/` paths. The
2026-08-20 backfill corrected `tasks.md` (T001/T004/T005/T006 → `lib/spdk-sys/`
plus a dated note at `tasks.md:10`) and `spec.md:6` (bracketed editorial note).
Verified still present. `spec.md:99,188` reference the unchanged crate *name*
(still resolves) and were correctly left unedited. No further action.

**approved**: true (already applied)

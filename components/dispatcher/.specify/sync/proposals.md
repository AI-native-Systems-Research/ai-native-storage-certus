# Spec Sync Proposals — dispatcher (Phase B)

Generated: 2026-08-20
Spec: components/dispatcher/specs/001-dispatcher-cache-interface/spec.md
Drift source: components/dispatcher/.specify/sync/drift-report.{json,md} (3 drifted, 0 not-implemented, 1 unspecced group)

Each drift item was classified by reading the code at its `location`. The code is the
working, intentional reality in every case; no code change is proposed (no ALIGN items).
Two of the three drift items are documentation drift in the component `CLAUDE.md`, which is
outside this sync's editable scope (edits are restricted to `.specify/sync/` and `specs/`);
those are recorded as BACKFILL proposals but deferred to a follow-up doc pass.

---

## BACKFILL-US011 — Batch cold-promotion per-thread queue depth (US-11 → 128)

- **Direction**: BACKFILL (spec → matches code)
- **Requirement**: User Story 11 (narrative + acceptance scenario 3) / FR-039
- **Drift ref**: US-011 / FR-039 (moderate)
- **Location**: components/dispatcher/src/lib.rs:2217

**Before** —
- Narrative: *"Each thread uses a reduced NVMe pipeline depth (`16 / num_queues`) to share the drive's submission queue capacity without overflow."*
- Scenario 3: *"...each processed by a separate thread with `max_queue_depth = 8` (16/2), keeping total per-drive NVMe commands at ≤16."*

**After** —
- Narrative: *"Each thread drives its cold promotions with a deep per-thread NVMe pipeline depth (`max_queue_depth = 128`, per FR-039 step (5) and FR-019) to saturate the drive's submission queue for maximum per-drive parallelism."*
- Scenario 3: *"...each processed by a separate thread, and each thread drives its cold promotions with `max_queue_depth = 128` (per FR-039 step (5) / FR-019) to saturate the drive's NVMe submission queue."*

**Rationale** — `batch_lookup` sets `let queue_depth = 128;` (`src/lib.rs:2217`) and passes it into
`pipelined_multi_object_zero_copy`. This is deliberate and already correctly stated by FR-039 step (5)
and FR-019 ("`max_queue_depth = 128` per thread"). The User Story 11 narrative and scenario 3 carried
the older "16 / num_queues (=8), ≤16 per drive" wording, an intra-spec contradiction against the FRs
and the code. Spec-lag in the user-story text → BACKFILL the story text to match the aligned FRs/code.

---

## BACKFILL-DOC-fwpath — CLAUDE.md stale component-framework path (RECORDED, NOT APPLIED)

- **Direction**: BACKFILL (doc → matches reality)
- **Requirement**: CLAUDE.md crate-location note
- **Drift ref**: CLAUDE.md (stale crate path) (minor)
- **Location**: components/dispatcher/CLAUDE.md:40

**Before** — *"`component-framework`, `component-core`, `component-macros` — at `../../component-framework/crates/`"*

**After (proposed)** — *"`component-framework`, `component-core`, `component-macros` — at `../../../lib/component-framework/crates/` (moved from `components/` to `lib/`)"*

**Rationale** — After the repo move, `component-framework` lives at `lib/component-framework`
(`components/component-framework` no longer exists). `Cargo.toml` uses workspace deps
(`component-framework.workspace = true`), so the build is unaffected; only the doc path is stale.
Doc-lag → BACKFILL direction.

**Application status** — **NOT APPLIED.** `CLAUDE.md` is outside this sync's editable scope
(`.specify/sync/` and `specs/` only). Recorded here for a follow-up documentation pass.

---

## BACKFILL-DOC-v2names — CLAUDE.md stale `-v2` crate names (RECORDED, NOT APPLIED)

- **Direction**: BACKFILL (doc → matches reality)
- **Requirement**: CLAUDE.md dependency-crate names
- **Drift ref**: CLAUDE.md (stale crate names) (minor)
- **Location**: components/dispatcher/CLAUDE.md:43-44,53

**Before** — references to `block-device-spdk-nvme-v2` and `extent-manager-v2`.

**After (proposed)** — `block-device-spdk-nvme` and `extent-manager` (no `-v2` suffix; matching
`components/dispatcher/Cargo.toml:15`).

**Rationale** — There is no `-v2` suffix in the current workspace; the actual dependency crate is
`block-device-spdk-nvme`. Documentation-only drift → BACKFILL direction.

**Application status** — **NOT APPLIED.** `CLAUDE.md` is outside this sync's editable scope. Recorded
for a follow-up documentation pass.

---

## BACKFILL-UNSPECCED-057 — Dependency-injection / test hooks (new FR-057 + SC-016)

- **Direction**: BACKFILL-UNSPECCED (add new requirement to existing spec)
- **Requirement**: NEW FR-057 (+ SC-016)
- **Drift ref**: unspecced (`set_block_device_factory`, `set_extent_manager_factory`, `set_pipeline_metrics`)
- **Location**: components/dispatcher/src/lib.rs:358-374 (types at :203, :224; fields at :254-255)

**Before** — No requirement. FR-043 covers the `PipelineMetrics` *trait* but not the injection
setters; the block-device / extent-manager factory setters are entirely unspecced.

**After** — Add FR-057 documenting the three inherent `DispatcherComponent` DI setters
(`set_block_device_factory(BlockDeviceFactory)`, `set_extent_manager_factory(ExtentManagerFactory)`,
`set_pipeline_metrics(Arc<dyn PipelineMetrics>)`) as the test/DI surface that overrides the
internally-constructed dependencies, with fallback to the default hard-coded implementations when a
factory is not set. Add SC-016 as its measurable outcome (exercise the data path and observe pipeline
timings with mocks injected, no NVMe hardware).

**Rationale** — These public inherent methods ship and are used for hardware-free component testing
(the factories back the `MockBlockDevice`/mock extent-manager test paths; `set_pipeline_metrics` backs
telemetry capture). They are inherent methods on the concrete component, not `IDispatcher` trait
methods, matching how FR-001 already documents `create_eviction_channel`/`eviction_dropped_count`.
Code is authoritative → backfill a "test/DI surface" requirement rather than gate the methods behind
`#[cfg(test)]` (they are used as a public injection API).

---

## Summary

| Proposal | Direction | Applied |
|---|---|---|
| BACKFILL-US011 | BACKFILL | Yes (spec.md) |
| BACKFILL-DOC-fwpath | BACKFILL | No — CLAUDE.md out of scope |
| BACKFILL-DOC-v2names | BACKFILL | No — CLAUDE.md out of scope |
| BACKFILL-UNSPECCED-057 | BACKFILL-UNSPECCED | Yes (spec.md: FR-057 + SC-016) |

No ALIGN, RESOLVED, or HUMAN_DECISION items this run.

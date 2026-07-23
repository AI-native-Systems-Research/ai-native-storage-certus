# Spec Sync — Align Tasks
Project: dispatcher-p2p
Source drift cycle: 2026-07-22T22:35:42Z (`drift-report.json` / `drift-report.md`)

Items below were deferred during AUTO-BACKFILL apply (2026-07-22) because resolving them
requires information/decisions beyond what is verifiable from `src/` and existing specs alone.

---

## Task: Align 001-gpudirect-cold-path/FR-017

**Severity**: Low (AMBIGUOUS)

**Spec Requirement**: FR-017 (newly backfilled) documents the eviction-event channel
(`create_eviction_channel`, `EvictionEvent`/`EvictionReason`, `eviction_dropped_count`) purely
in terms of its producer-side contract: bounded channel, single subscriber, non-blocking
`try_send`, drop-and-count backpressure. It intentionally does NOT name a consumer.

**Current Code**: `src/lib.rs:213-232` (`create_eviction_channel`, `eviction_dropped_count`,
`emit_eviction`) and `:546-650` (`evict_for_space_inner`/`evict_for_space_emit` emission sites)
implement the producer side only. No call site of `create_eviction_channel` exists within
`components/dispatcher-p2p` itself — the drift report (`drift-report.md` §Unspecced Code)
speculates the consumer is "plausibly the gRPC `TakeEvents` RPC added in sibling commit
`4d5bd13`" but this was not verified against that commit or against any wiring code in this
component's tree.

**Required Change**: A human (or a follow-up sync pass with access to the consuming
component/service, e.g. the server crate implementing `TakeEvents`) should confirm:
1. Whether `create_eviction_channel` is actually called anywhere today, and by what component.
2. Whether the RPC/consumer has its own delivery-semantics expectations (ordering, at-most-once
   vs. best-effort, capacity sizing) that should be reflected back into FR-017 or cross-referenced
   from it.
3. Whether this warrants promoting eviction-event delivery into its own spec (e.g.
   `002-eviction-telemetry`) once a real consumer contract exists, rather than living as a single
   FR inside `001-gpudirect-cold-path`.

**Files to Modify**: `components/dispatcher-p2p/specs/001-gpudirect-cold-path/spec.md` (FR-017),
`components/dispatcher-p2p/specs/001-gpudirect-cold-path/data-model.md` (`EvictionEvent /
EvictionReason` entity) — pending confirmation of consumer contract; possibly a new spec directory
if eviction telemetry grows its own acceptance scenarios/success criteria.

---

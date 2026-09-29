# Implementation Plan: Serving-Tier Attribution (`served_by`)

**Branch**: `certus-lookup-observability` | **Date**: 2026-09-25 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/002-served-by-tier-attribution/spec.md`

**Scope of this plan revision**: **Phase 1 only — the counters.** The spec covers five units
and an interface change; this plan commits to the smallest slice that is independently
useful, and declares the rest as later phases rather than planning them now. The reason is in
`research.md` R1: a remote-hit counter needs no interface change, no wire change, and no
change to four of the five units.

## Summary

Make the servers' lookup accounting complete and add the counter that does not exist: how
many lookups were served by a **peer** rather than locally. Today nothing can separate a
local hit from a remote one, so remote lookup's benefit is unmeasurable — two hardware runs
established its cost and neither could establish its benefit.

Phase 1 delivers three things inside `dispatcher` and `certus-server-yaml`, with no
interface or wire change:

1. A requester-side remote-hit counter, incremented at the one site where the dispatcher
   already knows the answer (`lib.rs:2575`; the other two `remote_lookup.get()` guards are
   `join_cluster` and `leave_cluster`, not lookups).
2. Closure of the two confirmed accounting holes, so hits plus misses plus errors equals
   entries requested (FR-024).
3. Restoration of the miss classification the dispatcher currently destroys — the **confirmed**
   cause of the zero reading (`research.md` R2): a peer miss is relabelled `IoError`, which the
   counter then drops. Plus a measurement that confirms it against a stated prediction rather
   than merely observing the number move.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**: `component-framework`, `interfaces`, `shmq-dispatcher`,
`opentelemetry` (server-side export only). No new dependency.
**Storage**: none — counters are process-local `AtomicU64`.
**Testing**: `cargo test -p dispatcher`, `-p shmq-dispatcher`, `-p certus-server-yaml`; plus
one hardware measurement on node2 that the unit tests cannot make.
**Target Platform**: Linux x86-64. The dispatcher selection is a build-time profile
(`CERTUS_PROFILE`), so `dispatcher` and `dispatcher-p2p` are separate build configurations
rather than a runtime switch — Phase 1 touches only `dispatcher`.
**Project Type**: single component plus its transport host.
**Performance Goals**: no measurable per-operation cost. A `Relaxed` `fetch_add` on an
existing hot path is the budget; anything requiring a lock or an allocation is out of scope
by construction.
**Constraints**: `TranslatorObserver`'s methods must stay defaulted, or the plain
`certus-server` binary and every test host implementing the trait break (`research.md` R5).

## Constitution Check

**There is no constitution to check against, and that is a finding rather than a pass.**

`components/dispatcher` has never had `.specify/memory/constitution.md` on any ref. Feature
001's plan nonetheless runs a Constitution Check citing seven principles —
`II. Interface-Only Public API`, `IV. Performance Assurance`, `VII. Linux-Only Platform` and
others — and **none of the 15 component constitutions in this repository matches that set**;
the repo-root constitution is still an unfilled `[PROJECT_NAME]` template.

So 001's gate was asserted against principles that were never written down. This plan does
not repeat that. It records the state and defers the decision:

- Adopting a sibling's constitution (`dispatcher-p2p`'s) would give a real gate but would
  make 001 and 002 cite two different principle sets inside one component.
- Authoring one for `dispatcher` is the correct fix and is not this plan's to make.

**Pending decision**, carried explicitly rather than silently passed. The obligations this
plan does commit to are the ones the spec states (FR-024..FR-032) and the component-doc rule
below, which is the one that actually has teeth here.

### Per-component documentation

Phase 1 changes two units. Neither is verification-bearing, which is why it is a safe first
slice:

| Unit changed | Spec to update | Creusot/Spin |
| --- | --- | --- |
| `components/dispatcher` | `001-dispatcher-cache-interface`, and this spec | no |
| `lib/shmq-dispatcher` | owns no `specs/`; document at the definition | no |
| `apps/certus-server-yaml` | owns no `specs/` | no |

Later phases reach `components/dispatcher-p2p` and `components/remote-lookup`, **both of
which carry Creusot and Spin tooling** — so their specs are not documentation but the artifact
a proof is written against (FR-030). That is a reason to keep them out of Phase 1, not merely
a consequence of doing so.

## Project Structure

### Documentation (this feature)

```text
components/dispatcher/specs/002-served-by-tier-attribution/
├── spec.md              # reconciled 2026-09-25; the whole feature
├── plan.md              # this file — Phase 1 scope
├── research.md          # R1-R5: the independence finding and the zero-reading causes
├── tasks.md             # Phase 1 tasks (next step)
├── checklists/
│   └── requirements.md
└── contracts/
    ├── idispatcher.md   # the interface delta — Phase 2, not Phase 1
    └── served-by.md     # taxonomy + control-plane surface; carries one open decision
```

### Source Code (repository root)

```text
components/dispatcher/src/lib.rs        # count + reclassify at lib.rs:2575
lib/shmq-dispatcher/src/translate.rs    # close both accounting holes; new observer method
apps/certus-server-yaml/src/
├── metrics.rs                          # ServiceCounters gains the counters
├── telemetry.rs                        # export them
└── main.rs                             # no change expected; wiring already exists
```

## Phasing

**Phase 1 — counters (this plan).** No interface change, no wire change, `dispatcher` only.
Answers "does remote lookup serve anything".

**Phase 2 — the interface.** `ServedBy` in `interfaces`, `batch_lookup`'s return type, and
**every implementor, because the compiler leaves no choice** — see the boundary note below.
Re-counted 2026-09-28: four impls, of which **two are production** (`components/dispatcher`,
`components/dispatcher-p2p`) and two are mocks (`components/remote-lookup/src/seams.rs`,
`lib/shmq-dispatcher`'s test mock). The inventory in `contracts/idispatcher.md` still
predates two upstream changes and its line numbers remain stale.

Phase 2 also carries **the `IRemoteLookup` delta** (`contracts/served-by.md`, "The
`IRemoteLookup` delta"), which was previously filed under Phase 4. It cannot wait: the
projection keeps `RemoteDram` (`3`) and `RemoteSsd` (`4`) distinct, and the dispatcher can
only tell them apart from the peer's advertised tier carried out of
`IRemoteLookup::batch_lookup`. Without the delta, Phase 2 would have to either invent a
remote value or collapse the two — both of which Phase 3 would then have to undo.

**Phase 3 — the control plane.** Widen `LOOKUP`'s per-key byte. Carries the open decision in
`contracts/served-by.md` about the five-value projection, which needs sign-off before it is
built.

**Phase 4 — verification and the component specs, NOT "making the other dispatchers work".**
They already work after Phase 2; what they lack is proof and specification. Phase 4 owns:
the verification-bearing feature specs for `dispatcher-p2p` and `remote-lookup`; the FR-027
and FR-028 tests for attribution those components produce; and specifically the FR-014
cold-path case, which spec.md records **cannot be tested by flipping a flag** and so needs
real test construction rather than a parameter.

Phase 1 is deliberately ordered first because it is the only phase that produces a
**measurement** rather than a capability, and the measurement may change what later phases
are worth.

### Boundary note: why Phase 4 cannot own the other implementors

Widening `batch_lookup`'s return type is a **signature** change, so all four implementors
become compile errors the moment Phase 2 lands. Deferring any of them to Phase 4 would leave
the workspace unbuildable in between, which no phase boundary may do.

So Phase 2 necessarily touches `dispatcher-p2p`, and it must give it the **right** value
rather than a placeholder: FR-014 already fixes that value (its SSD-to-GPU cold path
attributes as `SSD`, because it does not populate DRAM synchronously). A placeholder would
be inventing an attribution the spec has already decided, and FR-027 forbids shipping a
value no test can fail against.

The earlier wording — "Phase 4 — the other dispatchers and remote-lookup" — contradicted
Phase 2's own "every implementor" and implied a phase ordering the compiler forbids.
Corrected 2026-09-28.

## Complexity Tracking

| Item | Why needed | Simpler alternative rejected because |
| --- | --- | --- |
| A new `TranslatorObserver` method | The remote count is known in `dispatcher`, but counters live in the server; the observer is the existing seam between them | Returning it through `batch_lookup` is the Phase 2 interface change this phase exists to avoid. Reading it from a global would make two servers in one process share a counter |
| Measuring with a prediction, not merely after the fix | R2's cause is now confirmed by code reading, so the measurement becomes a test of understanding: solo must read non-zero misses and shared zero, on one workload | Observing only that the number moved cannot distinguish "we understood it" from "we changed something adjacent". If the prediction fails, a second defect is hiding behind the first |
| Counting requester-side only | It is what the open question needs: what this node obtained from peers | A responder-side counter measures what peers asked of this node. Both are useful; conflating them under one name would produce a metric nobody can interpret |

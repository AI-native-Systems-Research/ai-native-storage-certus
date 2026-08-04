# Specification Quality Checklist: Serving-Tier Attribution (`served_by`)

**Purpose**: Validate specification completeness and quality before proceeding to planning.

**Created**: 2026-08-04

**Feature**: [spec.md](../spec.md)

## Content Quality

- [X] No implementation details leak into requirements — FRs state *what* must be attributed
      and with what invariants, not how the value is threaded
- [X] Focused on measurable outcomes and their consumer, not on code structure
- [X] Written so a reader who has not read the dispatcher can follow it
- [X] All mandatory sections completed
- [ ] Note: the spec cites `file:line` throughout. This is deliberate — the feature's whole
      claim is that the datum already exists at specific sites — but those citations will go
      stale as the files move. They are evidence, not requirements

## Requirement Completeness

- [X] No `[NEEDS CLARIFICATION]` markers remain
- [X] Requirements are testable and unambiguous
- [X] Success criteria are measurable
- [X] Success criteria are technology-agnostic (no Rust or proto types in SC-001..SC-009)
- [X] All acceptance scenarios are defined
- [X] Edge cases identified — including the two overwrite paths, concurrent promotion,
      staging, single-flight followers, and the `AlreadyExists` publish path
- [X] Scope is bounded, with an explicit `## Out of Scope` naming eight excluded items
- [X] Dependencies and assumptions identified, per affected component

## Feature Readiness

- [X] All functional requirements have clear acceptance criteria
- [X] User scenarios cover the primary flows (interface, gRPC, remote, accounting, both
      dispatchers)
- [X] Feature meets the measurable outcomes defined in Success Criteria
- [X] No implementation leakage into the success criteria

## Open Risks Carried Into Planning

- [ ] **Mock fidelity.** FR-028 exists because this repo has already shipped vacuous
      assertions: `MockDispatchMap` had two compensating infidelities that made every pin
      assertion pass while asserting nothing. Attribution tests are exposed to the same
      failure mode — a mock that returns a plausible tier without modelling residency proves
      nothing. Every attribution test must be observed to fail against a deliberately wrong
      attribution before it is trusted
- [ ] **Downstream spec conflict.** The workload-generator spec assumes a five-value taxonomy
      (its FR-039, US3 acceptance 1, and the `Outcome` entity). This feature specifies seven.
      That spec must be updated; the conflict is recorded rather than reconciled silently
- [ ] **Remote attribution is an advertisement, not ground truth.** A small fraction of
      remote attributions can be wrong in a way this feature cannot detect. Accepted for
      aggregate hit rate; inadequate for per-request forensics. Any report built on it should
      say so
- [ ] **`REMOTE_SSD` is transient.** Serving from a peer's disk promotes the entry into that
      peer's DRAM, so a fixed holder configuration yields a decaying `REMOTE_SSD` fraction
      rather than a stable one. A test asserting a stable fraction would be wrong, not the code
- [ ] **Python stub staleness.** Two of the three checked-in stub sets are already drifted;
      one is shipped to remote nodes by the multi-node test script. Regeneration is in scope
      as a decision (FR-030) but the drift repair is not

## Notes

- Filed under `components/dispatcher` because that is where the tier is resolved, and because
  this repo specifies `interfaces`-crate changes in the consuming component's spec rather
  than under `components/interfaces/specs/`. `apps/certus-server-yaml` cannot own it — it has
  no `specs/` or `.specify/` tree.
- Three design decisions were taken by the requester on 2026-08-04 and are recorded in
  `## Clarifications`: advertised remote tier rather than serve-time ground truth;
  `SIZE_MISMATCH` as its own bucket; and fixing the hits/misses accounting defect while
  deferring the `rw-telemetry` and `GetIoStats` items. The seventh value, `ERROR`, is a
  derived consequence of the third and is flagged as such.

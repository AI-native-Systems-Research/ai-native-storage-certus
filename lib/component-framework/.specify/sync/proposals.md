# Drift Resolution Proposals

Generated: 2026-09-02T21:46:38Z
Based on: drift-report generated 2026-09-02T21:46:38Z (git 2fc1cd3c)

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 0 |
| Align (Spec -> Code) | 1 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 0 |

148 of 149 requirements are aligned. The single functional drift is resolved
via ALIGN (the specs already state the intended behavior correctly, so no spec
text changes). Two trivial doc-only path references are housekeeping.

---

## Proposal P-001 — ALIGN: actor re-activation must not panic

**Direction**: ALIGN (Spec -> Code). **Approved**: true (recorded as an
align-task; no code edited by spec-sync per constraints).

**Drifted requirements**: 005-numa-aware-actors/FR-001, 003-actor-channels/FR-004.

**Finding**: The specs state actors are single-use and that lifecycle misuse is
reported via `Result::Err` (or prevented at the type level), never a panic
(005/spec.md:82,90; 003/spec.md:127). The implementation's
`Actor::activate(&self)` (`crates/component-core/src/actor.rs:589`) takes
`&self` and does not consume the actor; `deactivate()` resets `state` to
`STATE_IDLE` (`actor.rs:229`), so a second `activate()` passes the CAS and then
**panics** via `.expect(...)` on the already-taken receiver/handler
(`actor.rs:610`, `actor.rs:617`).

**Proposed code change** (NOT applied by spec-sync — appended to
`align-tasks.md` for a human decision): either
(a) return a typed error before spawning (e.g. new `ActorError::AlreadyConsumed`,
or reuse `AlreadyActive` with updated docs) when the receiver/handler has
already been taken, or
(b) change the signature to `activate(self)` so re-use is impossible at compile
time (matches the spec's "consumed on activation" wording).

**Why ALIGN and not BACKFILL**: the spec already describes the correct,
safe behavior; the code is the side that diverges. Rewriting the spec to bless
a panic would be a guarantee-violation backfill and is disallowed.

**Confidence**: HIGH (behavior verified from source; item independently
identified on 2026-07-22 and still open).

---

## Proposal P-002 — Fix stale relocation path in align-tasks.md

**Direction**: Housekeeping (sync-artifact doc fix). **Approved**: true (applied
this run).

`align-tasks.md:29` "Files to Modify" pointed at the pre-relocation path
`components/component-framework/crates/component-core/src/actor.rs`. Updated to
`lib/component-framework/crates/component-core/src/actor.rs`.

The equivalent stale path in `apply-report.md:60` sits inside a dated
historical "Next Steps" log entry and is left unmodified (rewriting historical
logs would misrepresent what that run recorded); it is noted in the drift
report instead.

---

## Housekeeping (carried forward, no drift)

- REC-001 (specs 001-006 status → Complete) and REC-002 (remove 006 backfill
  notice) from the 2026-04-10 proposals were already applied; specs now read
  `Status: Complete` and 006 has no backfill notice. No further action.

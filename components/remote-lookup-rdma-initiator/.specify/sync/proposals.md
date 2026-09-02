# Drift Resolution Proposals — remote-lookup-rdma-initiator

**Generated**: 2026-09-02T21:46:01Z
**Based on**: `.specify/sync/drift-report.md` (2026-09-02), git `2fc1cd3c`

## Summary

| Resolution Type | Count | Status |
|-----------------|-------|--------|
| Backfill (Code → Spec) | 0 | - |
| Align (Spec → Code) | 1 | ✅ APPROVED (queued; code edit out of scope) |
| Human Decision Required | 0 | - |
| New Specs | 0 | - |

Spec-002's text is accurate against the implementation, so there is no BACKFILL
to apply. The one open item is a code-side doc-comment fix (ALIGN), which cannot
be applied by a Markdown-only sync pass and is therefore recorded in
`align-tasks.md` for a maintainer to land as an ordinary code change.

---

## Proposal 1: 002-rdma-push-initiator / SC-004 — stale benchmark header comment

**Status**: ✅ **APPROVED** (as an ALIGN task; not applied here — `.rs` edit)

**Spec**: `specs/002-rdma-push-initiator/spec.md` SC-004 (lines 316-329)
**Code**: `benches/push_telemetry.rs:1-18` (header doc comment, esp. lines 3-4, 17-18)

**Direction**: ALIGN (spec → code). The spec is correct; the benchmark's header
doc comment is stale.

**Current State**:
- **Spec says** (SC-004, reframed 2026-07-15): telemetry-on cost must be a "small
  fixed absolute cost" — a handful of `Relaxed` atomic updates — and a ZST no-op
  when off; a naive `<5%`-of-mock gate is explicitly rejected because the mock
  push is a ~200–700 ns no-op that makes unavoidable atomics read as 6–13%.
- **Code comment does**: still states "requires that enabling the `telemetry`
  feature adds less than 5% overhead to `push`" and "SC-004 holds when every
  `push/*` case is within +5%." The benchmark *mechanics* (two-baseline on/off
  workflow over the mock transport) and the shipped telemetry behavior are
  correct — only the stated pass/fail bar in the comment is outdated.

**Proposed Resolution**: Reword the `benches/push_telemetry.rs` header comment to
the "small fixed absolute cost / ZST-when-off" criterion and cross-reference
SC-004's 2026-07-15 measurement note, so a contributor running the benchmark does
not chase a bar the spec itself says is unmeetable against the mock baseline. No
benchmark logic change.

**Confidence**: HIGH | **Effort**: small | **Severity**: minor | **Risk**: NONE

**Why not applied this pass**: This is a Rust source file, out of scope for a
Markdown-only spec-sync-apply (hard rule: edit only spec `.md` and
`.specify/sync/**`). Carried in `align-tasks.md` (Task A), where it has been
queued since the 2026-07-22 and 2026-08-07 passes.

---

## Notes on items considered and NOT proposed

- **Spec-001 (superseded)**: its 23 FRs/SCs describe the removed passive-responder
  design; no align/backfill — the role was wholesale-replaced by spec-002. Its two
  stale FR-014/FR-015 self-annotations are already annotated in place. Full
  retirement to an archive path is a maintainer follow-up, not a sync proposal.
- **`tests/mr_registration_bench.rs`**: the July "DEFERRED" human-decision item
  was resolved on 2026-08-07 by backfilling a Known-Limitations line in spec-002;
  nothing outstanding.
- **`src/rdma.rs` / `ffi.rs` / `wrapper.c`, `src/loopback_test.rs`**: covered by
  spec-002 Assumptions (`rdma` feature) and Known Limitations (validation tooling)
  respectively — no proposal.

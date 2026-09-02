# Spec Sync — Align Tasks

Generated: 2026-07-22T23:21:48Z
Component: remote-lookup-rdma-initiator
Source: `.specify/sync/drift-report.md` (2026-07-22), applied via AUTO-BACKFILL sync-apply.

Note: spec-001's 23 "Not Implemented" requirements (FR-001..FR-017, SC-001..SC-006)
are **not** listed here — they describe the passive-responder design that spec-001
itself marks superseded-by-design; there is nothing to align, the role was replaced
wholesale by spec-002. See spec-001's banner and the drift report's
"Not Implemented (obsolete by design)" table for the full list.

---

## Task: Align 002-rdma-push-initiator/SC-004 — stale benchmark header comment

**Spec Requirement**: SC-004 (telemetry overhead)

**Current Code**: `benches/push_telemetry.rs:1-18`'s header doc comment still states
the pre-revision pass/fail bar verbatim: "SC-004 requires that enabling the
telemetry feature adds less than 5% overhead to push versus the disabled build
... SC-004 holds when every push/* case is within +5%."

**Required Change**: Update the header comment to match the spec's 2026-07-15
revision — SC-004 is now "small fixed absolute cost / ZST-when-off", not a literal
<5%-of-mock gate (a straight <5% read against the ~200–700ns mock push cannot hold
by construction; measured on/off deltas were +8–13% at push/16..64). Reword the
comment so a future contributor running the benchmark doesn't chase a bar the spec
itself says is unmeetable against the mock baseline.

**Files to Modify**: `components/remote-lookup-rdma-initiator/benches/push_telemetry.rs`
(lines 1-18, doc comment only — no benchmark logic changes)

**Estimated Effort**: small

**Severity**: minor (doc-comment only; benchmark mechanics and shipped behavior
already match the spec — only the stated pass/fail bar in the comment is outdated)

**Note**: This file is Rust source, not Markdown, so it is out of scope for this
spec-sync-apply pass (hard rule: edit only Markdown under `specs/**` and
`.specify/sync/**`). Tracked here as a follow-up code change.

### Acceptance Criteria
- [ ] `benches/push_telemetry.rs` header comment states the "fixed absolute cost /
      ZST-when-off" criterion instead of the literal "<5% vs disabled" framing
- [ ] Comment cross-references `specs/002-rdma-push-initiator/spec.md` SC-004's
      2026-07-15 measurement note

---

## Task: DEFERRED — spec note for `tests/mr_registration_bench.rs` investigation

**Spec Requirement**: None currently (unspecced code, drift-report "Unspecced Code"
table, item 2)

**Current Code**: `components/remote-lookup-rdma-initiator/tests/mr_registration_bench.rs`
(248 lines) — an `#[ignore]`d, `--features rdma`-gated hardware benchmark sweeping
`ibv_reg_mr` cost by pool size/page type, to inform whether FR-004's per-connection
pool MR re-registration should become a single shared MR. Its own header already
frames this as an open design trade-off.

**Required Change**: Unresolved. Two options were identified by the drift report:
(a) add a line to `002-rdma-push-initiator/spec.md`'s Known Limitations section
referencing this investigation as informing FR-004, or (b) leave it as internal
tooling with a comment noting it is deliberately unspecced research, not a
requirement. This task's directions did not specify which; **deferred** rather
than guessed.

**Files to Modify**: possibly `specs/002-rdma-push-initiator/spec.md` (Known
Limitations section) — no source file changes implied either way.

**Estimated Effort**: small

**Severity**: low (informational; no drift, no incorrect spec text — purely a
"should this be mentioned" documentation question)

### Acceptance Criteria
- [ ] Human decision: does `tests/mr_registration_bench.rs` warrant a Known
      Limitations line in spec-002, or is it fine to remain unspecced research?
- [ ] If yes, add one bullet under spec-002 "Known Limitations / Follow-ups"

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`)

Spec-002 (design-of-record) shows **zero drift** — no new ALIGN items. The two
tasks from the July pass are reconciled below; no new code-align work surfaced.
All resolvable spec-side items were BACKFILLED and applied (see apply-report.md
2026-08-07 section).

## Task A — (carried forward) SC-004 stale bench header comment (Low)

- **Status**: still **queued, not drafted** (unchanged from July align-task 1).
- **Spec**: `002-rdma-push-initiator` SC-004.
- **Current code**: `benches/push_telemetry.rs:1-18` header comment still states
  the pre-revision literal `<5% vs disabled` pass/fail bar; spec-002 SC-004 was
  reframed (2026-07-15) as "small fixed absolute cost / ZST-when-off". Benchmark
  mechanics and shipped behavior already match the spec — only the comment's
  stated bar is outdated.
- **Required change**: reword the header comment to the fixed-absolute-cost /
  ZST-when-off criterion and cross-reference SC-004's 2026-07-15 note.
- **Files**: `components/remote-lookup-rdma-initiator/benches/push_telemetry.rs`
  (doc comment only; no benchmark logic).
- **Why queued, not drafted**: Low severity, doc-comment only — per the sweep
  pacing, only HIGH code bugs get a drafted fix; this stays a queued task.
- **Owner**: remote-lookup-rdma-initiator maintainer.

## Task B — (resolved this pass) mr_registration_bench Known-Limitations note

- **Status**: **RESOLVED** — the July "DEFERRED" human-decision item is closed.
- The `tests/mr_registration_bench.rs` (and `src/loopback_test.rs`) validation
  tooling is now documented under spec-002's Known Limitations as deliberately
  unspecced engineering/validation tooling (option (a) from the July deferral).
  No source change; no further action.

## Note — spec-001 stale annotations (FR-014, FR-015)

Not align-tasks (spec-001 is superseded; there is no code to align to it). The
two stale self-annotations were **annotated in place** this pass to mark them as
describing the opposite of current code (telemetry IS integrated; trait methods
are functional, no `serve` module exists). Full retirement of spec-001 to an
archive path is a maintainer follow-up, tracked in apply-report.md Next Steps.

---

# 2026-09-02 Re-analysis (git 2fc1cd3c)

Full re-verification of spec-002 against current `src/`/`tests/`/`benches/`
(source unchanged since 2026-07-30, commit `00bd4002`). All 22 shipped-behavior
FRs/SCs remain aligned at their cited `file:line`. **One ALIGN item is still
open** and carried forward unchanged; no new align work surfaced.

## Task A — (still open, carried forward) SC-004 stale bench header comment (minor)

- **Status**: **still queued, not drafted** — unchanged since the 2026-07-22 and
  2026-08-07 passes. Verified 2026-09-02 that `benches/push_telemetry.rs:1-18`
  still states the literal `<5% vs disabled` bar (lines 3-4, 17-18).
- **Spec**: `002-rdma-push-initiator` SC-004 (spec.md:316-329), reframed
  2026-07-15 to "small fixed absolute cost / ZST-when-off".
- **Required change**: reword the header comment to the fixed-absolute-cost /
  ZST-when-off criterion and cross-reference SC-004's 2026-07-15 note; no
  benchmark logic change.
- **Files**: `components/remote-lookup-rdma-initiator/benches/push_telemetry.rs`
  (doc comment only).
- **Why not applied here**: `.rs` file — out of scope for a Markdown-only
  spec-sync-apply. Because this remains actionable, this pass reports
  `drift_status: drift` (honest), rather than the 2026-08-07 "clean" verdict that
  folded the same open task under a clean headline.
- **Owner**: remote-lookup-rdma-initiator maintainer.

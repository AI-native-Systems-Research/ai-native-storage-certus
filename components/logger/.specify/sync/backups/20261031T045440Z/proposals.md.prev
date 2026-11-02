# Drift Resolution Proposals

Generated: 2026-08-20
Based on: drift-report (components/logger/.specify/sync/drift-report.json)

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 1 |
| Align (Spec -> Code) | 0 |
| Backfill Unspecced | 0 |
| Resolved (already fixed) | 0 |
| Human Decision | 0 |

## Proposals

### Proposal 1: 001-logger-component/FR-006

**Direction**: BACKFILL (Code -> Spec)

**Requirement**: FR-006 — Console color codes per log level.

**Current State**:
- Spec says: "Console output MUST include ANSI color codes to distinguish log
  levels (e.g., red for error, yellow for warn, green for info, blue/cyan for
  debug)."
- Code does: `LogLevel::ansi_color` (`src/lib.rs:84-90`) maps Error →
  `\x1b[31m` (red), Warn → `\x1b[38;5;208m` (256-color orange), Info →
  `\x1b[32m` (green), Debug → `\x1b[36m` (cyan). The unit test
  `test_all_levels_colored` (`src/lib.rs:378`, assertion at `src/lib.rs:386`)
  asserts the orange escape `\x1b[38;5;208m` for warn.

**Proposed Resolution**:
Rewrite FR-006 to name the exact escape codes the implementation emits,
replacing "yellow for warn" with the 256-color orange `\x1b[38;5;208m`, and
pin the error/info/debug codes to the values in `ansi_color`. Add an
acceptance scenario under User Story 1 asserting the orange warn prefix and
reset code so the requirement is verifiable against `test_all_levels_colored`.

**Before** (spec text):
> **FR-006**: Console output MUST include ANSI color codes to distinguish log
> levels (e.g., red for error, yellow for warn, green for info, blue/cyan for
> debug).

**After** (spec text):
> **FR-006**: Console output MUST include ANSI color codes to distinguish log
> levels: red (`\x1b[31m`) for error, orange (256-color `\x1b[38;5;208m`) for
> warn, green (`\x1b[32m`) for info, and cyan (`\x1b[36m`) for debug.

**Rationale**: This is a cosmetic spec/impl mismatch (severity: minor). The
warn color was intentionally chosen as 256-color orange and is locked in by a
passing unit test; the code is the working, intended reality. The spec's
generic "yellow"/"blue/cyan" wording is stale. BACKFILL brings the spec to the
code per the Phase B default direction (spec-lag → BACKFILL).

**Confidence**: HIGH

**Action**:
- [x] Approved (auto-approved per Phase B shared policy: spec-lag backfill)

---

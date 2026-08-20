# Align Tasks — dispatcher (Phase B)

Generated: 2026-08-20
Source: `drift-report.{json,md}` (3 drifted, 0 not-implemented, 1 unspecced group)

**No ALIGN tasks this run.**

Every drift item was classified by reading the code at its `location`, and in every case the
code is the working, intentional reality (spec-lag / doc-lag), not a behavioral bug:

- **US-011 / FR-039** (per-thread queue depth) — code uses `queue_depth = 128` (`src/lib.rs:2217`),
  which already matches FR-039 step (5) and FR-019. The stale text was in the User Story 11 narrative
  and acceptance scenario 3 → resolved by **BACKFILL** to the spec.
- **CLAUDE.md stale crate path** and **CLAUDE.md stale `-v2` names** — documentation-only drift; code
  and build are correct → **BACKFILL** direction, but the target (`CLAUDE.md`) is outside this sync's
  editable scope (`.specify/sync/` and `specs/` only), so they are recorded in `proposals.*` and
  deferred to a follow-up doc pass, not applied.
- **Unspecced DI/test hooks** — working public inherent API → **BACKFILL-UNSPECCED** (new FR-057 + SC-016).

No item genuinely violates a correct, agreed spec requirement, so no code-alignment task is required.

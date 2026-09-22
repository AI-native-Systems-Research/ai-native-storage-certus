# Align Tasks — dispatcher (code-side follow-ups)
Generated: 2026-08-31 (sync on branch `sync-tmp`)

This sync produced **no ALIGN items** (all six findings were code-authoritative
BACKFILL/BACKFILL-UNSPECCED, resolved by `specs/**` edits). The item below was a
documentation-only source cleanup that was outside this sync's editable scope
(`.specify/sync/**` + `specs/**`), recorded here for a follow-up source pass.

## T1 — ✅ RESOLVED (2026-09-21) — stale transport-example source comments removed

- **Status**: Resolved. No follow-up remains. The stale transport-specific
  comments in `copy_gpu_to_memory_async`'s null-stream branch were removed in
  commit `6d7ba234` ("revert dispatcher GPU experiments to specified stream
  model"). Verified 2026-09-21: `grep -rin grpc components/dispatcher/src`
  returns nothing, so the dispatcher source no longer references the removed
  control transport anywhere. The canonical spec's FR-040 / FR-042 already
  describe the shm-queue control transport (`2026-08-31` sweep).
- **Original item** (now closed): two comments described the null-stream caller
  with a transport-specific example that named the pre-`97e26738` control
  transport; the guidance was to reword to the shm-queue control handler or drop
  the example. Superseded by the `6d7ba234` deletion above.

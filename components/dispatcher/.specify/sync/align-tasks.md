# Align Tasks — dispatcher (code-side follow-ups)
Generated: 2026-08-31 (sync on branch `sync-tmp`)

This sync produced **no ALIGN items** (all six findings were code-authoritative
BACKFILL/BACKFILL-UNSPECCED, resolved by `specs/**` edits). The item below is a
documentation-only source cleanup that is outside this sync's editable scope
(`.specify/sync/**` + `specs/**`), recorded here for a follow-up source pass.

## T1 — Remove stale "gRPC handler" source comments — priority: low

- **File**: `components/dispatcher/src/lib.rs` (~line 2983, the null-stream branch of
  `copy_gpu_to_memory_async`; check for a second occurrence nearby).
- **What**: Two comments still say "e.g. gRPC handler" when describing the caller that
  passes a null stream. gRPC was removed in `97e26738` (shm-queue is the sole control
  transport). Reword to "e.g. the shm-queue control handler" or drop the transport-specific
  example.
- **Why**: Keeps source comments consistent with FR-040 / FR-042 (now shm-queue) and the
  `97e26738` transport change. No behavior impact.
- **Scope note**: Not applied by this sync — source files are outside the sync editing
  scope. Do in a normal source edit / PR.

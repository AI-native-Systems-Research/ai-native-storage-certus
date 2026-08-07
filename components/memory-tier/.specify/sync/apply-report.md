# Sync Apply Report

**Component**: memory-tier
**Mode**: AUTO-BACKFILL
**Applied**: 2026-07-22
**Source**: `.specify/sync/drift-report.json` / `.specify/sync/drift-report.md`

## Changes Made

### Specs Updated (BACKFILL — clearly-safe doc/text items)

| Spec | Item | Change Type | Notes |
|------|------|-------------|-------|
| 001-memory-tier | telemetry feature | Added | New User Story 7 + FR-027, FR-028, NFR-011 in `spec.md` documenting the `telemetry` Cargo feature (`MemoryTierTelemetry`/`TelemetrySnapshot`/`telemetry()`/`reset_telemetry()`/`telemetry_snapshot()`) that existed in code but was unspecced. |
| 001-memory-tier | `free_capacity()` | Added | New FR-029 in `spec.md` documenting `capacity() - used()` accessor. |
| 001-memory-tier | `DEFAULT_POOL_SIZE` unused constant | Documented | Noted as reserved-for-future in `spec.md` (Implementation Notes) and `plan.md` (Error Handling Strategy), matching the existing `NotEvictable` reserved-for-future pattern. |
| 001-memory-tier | Key Entities table | Added | `MemoryTierTelemetry` / `TelemetrySnapshot` rows added to `spec.md`. |

### New Specs Created

(None — no unspecced feature rose to the level of warranting a standalone spec; all were folded into the existing `001-memory-tier` spec per the "NEW_SPEC: none expected" directive.)

### Superseded

(N/A — no superseded specs for this component.)

### Deferred to Align-Tasks (judgment calls / code investigation — NOT applied to specs)

The following items describe an architectural gap between the spec/plan (16-way
sharded pool with Creusot-verified properties) and the actual implementation (single
unsharded `Pool`; `evict_lru_for_key` ignores its key argument; no Creusot artifacts;
three mismatched version strings). Per instructions, the spec's sharded design was
**left intact, not rewritten**, since it is unclear whether sharding was intentionally
dropped or is unfinished work. Full details, evidence, and both remediation options are
in `.specify/sync/align-tasks.md`.

| Align-Task | Severity | Spec Requirement(s) | Summary |
|---|---|---|---|
| sharding-not-implemented | High | FR-005, FR-006, FR-007, NFR-002, FR-013, FR-021, SC-3 | 16-way shard architecture (per-shard allocator/lock, key-modulo-16, round-robin/per-shard sampling) described in spec/plan does not exist; code uses one `RwLock<Pool>`. Two options noted: implement sharding, or backfill spec down to single-pool reality. |
| evict-lru-for-key-ignores-key | High | FR-014 | `evict_lru_for_key(key)` ignores `key` (bound as `_key`) and is a pure alias for `evict_lru()`. Resolution depends on the sharding decision above. |
| creusot-proofs-absent | Medium | SC-8 | No Creusot proof artifacts exist anywhere in the component; `IMemoryTier` interface doc comments still assert P4/P5/P10 "Verified" claims for behavior that doesn't exist in code. |
| version-mismatch | Medium | NFR-008 | Spec claims version `0.2.0`; `Cargo.toml` says `0.1.0`; `define_component!` macro says `0.3.0`. Not guessed — deferred pending release-history check. |
| readme-source-layout-drift | Low | (not a spec.md FR) | `README.md` describes a nonexistent `lru.rs`/`LruList` module. Out of scope for this pass (README.md is not under `specs/**`); flagged for a follow-up doc-only edit outside spec-sync. |

### Not Applied

All BACKFILL-eligible items were applied. No proposals were rejected.

## Backup

Original specs saved to `.specify/sync/backups/`:
- `spec.md.2026-07-22.bak`
- `plan.md.2026-07-22.bak`
- `tasks.md.2026-07-22.bak`

## Files Touched

- `components/memory-tier/specs/001-memory-tier/spec.md` (backfilled: telemetry, free_capacity, DEFAULT_POOL_SIZE note, Key Entities, Spec-Sync Notes callout)
- `components/memory-tier/specs/001-memory-tier/plan.md` (backfilled: DEFAULT_POOL_SIZE note, Spec-Sync Notes callout)
- `components/memory-tier/.specify/sync/align-tasks.md` (created)
- `components/memory-tier/.specify/sync/apply-report.md` (this file)
- `components/memory-tier/.specify/sync/backups/*.bak` (created)

No source code (`src/`, `Cargo.toml`, `README.md`) was modified, per hard rules for this
pass.

## Next Steps

1. Maintainer decision required on sharding: implement the 16-way shard design as
   originally specified, or formally retire it from `spec.md`/`plan.md`/
   `imemory_tier.rs`. See `.specify/sync/align-tasks.md` → "sharding-not-implemented".
2. Resolve `evict_lru_for_key()` dead-alias behavior once the sharding decision lands.
3. Decide on Creusot proof strategy (produce artifacts vs. remove "Verified" claims).
4. Reconcile `0.1.0` / `0.2.0` / `0.3.0` version strings against actual release history.
5. Optionally fix `README.md`'s stale `lru.rs` description (outside spec-sync scope;
   plain doc edit).

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`) — NO_ACTION (verify + note)

Mode: sweep re-analysis. Pacing: auto-apply safe BACKFILL; **defer HIGH
architectural decisions**. Regenerated drift report: 30 aligned, 9 drifted.
Every drifted item is already captured in the 2026-07-22 `align-tasks.md`
with recorded remediation options awaiting **maintainer sign-off**, so this
sweep applied **no new spec edits** and drafted **no code** — deliberately, per
the standing rule that architectural decisions already tracked/deferred are not
code bugs to draft.

## Verified still-tracked (no new action taken)

| Align-task | Severity | Status this sweep |
|---|---|---|
| sharding-not-implemented (FR-005/006/007, NFR-002, FR-013, FR-021, SC-3) | **High** | Still open. Spec's 16-way shard design vs. single unsharded `RwLock<Pool>`. Two options (implement / backfill-down) recorded; needs maintainer decision. **Not drafted** — architectural choice, not a mechanical bug fix. |
| evict-lru-for-key-ignores-key (FR-014) | **High** | Still open. `evict_lru_for_key(key)` binds `_key` and is a pure alias for `evict_lru()` (`src/lib.rs:465-467`). Resolution is coupled to the sharding decision; **remains tracked pending maintainer sign-off**. |
| creusot-proofs-absent (SC-8) | Medium | Still open. No Creusot artifacts exist; `imemory_tier.rs` doc comments still assert P4/P5/P10 "Verified" for behavior not in code. **SC-8 is a phantom "verified" claim** — flagged, not silently backfilled. |
| version-mismatch (NFR-008) | Medium | Still open. `Cargo.toml` 0.1.0 / `define_component!` 0.3.0 / spec 0.2.0 unreconciled; not guessed. |
| readme-source-layout-drift | Low | Still open. `README.md` describes a nonexistent `lru.rs`/`LruList`; outside `specs/**`. |

## Why NO_ACTION

Per sweep pacing, HIGH-severity items are only *drafted* when they are code
**bugs** with a clear correct fix. The two HIGH memory-tier items
(sharding, `evict_lru_for_key` key-ignore) are **design decisions** — whether
sharding was intentionally dropped or is unfinished is exactly the open
question the 2026-07-22 report refused to guess. Drafting either direction would
pre-empt a maintainer architectural choice, so both are left tracked. The
SC-8 phantom-Creusot claim and version mismatch likewise await the same
decision. No spec was rewritten to match the single-pool reality this sweep.

## Verification
- No files modified this sweep for memory-tier (verify-only). Existing `align-tasks.md` remains the authoritative open-item list.

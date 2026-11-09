# Spec-Sync Phase B — Proposals — block-device-filesys

**Generated**: 2026-08-20
**Based on**: `drift-report.json` (regenerated 2026-08-20)
**Backup**: `.specify/sync/backups/specs/001-block-device-filesys/spec.md.bak`

Drift report totals: 1 drifted requirement (FR-015), 0 not-implemented, 1 unspecced feature.

| Requirement | Direction | Severity | Status |
|---|---|---|---|
| FR-015 | BACKFILL | minor | APPLIED |
| FR-023 (new) | BACKFILL-UNSPECCED | low | APPLIED |

---

## Proposal 1 — FR-015 (BACKFILL)

**Direction**: BACKFILL (spec → matches code)

**Rationale**: The spec asserts that *both* `create()` and `DeviceConfig::new` "have runnable doc examples." Reading the `location` code (`src/lib.rs:77-81`) shows `create()`'s example is deliberately a ` ```ignore ` block — running it would allocate a real backing file on disk, so keeping it non-compiled is the intended, correct behavior. Only `DeviceConfig::new` (`src/config.rs:42-57`) is a truly runnable/compiled example. This is a stale/overclaiming spec against working, intentional code → BACKFILL (not ALIGN; nothing is behaviorally wrong).

**Before**:
> Key public API items (constructors, configuration types) MUST have documentation examples. The `create()` constructor and `DeviceConfig::new` have runnable doc examples. *(Corrected 2026-08-07 …)* `open_or_create_backing_file`, the interface method implementations (`IBlockDevice` impl block), and the lifecycle methods (`initialize`, `shutdown`) do not currently have individual doc examples — these are covered by integration tests instead.

**After**:
> Key public API items (constructors, configuration types) MUST have documentation examples. `DeviceConfig::new` has a runnable doc example (` ``` `) that exercises both valid and invalid configurations and is therefore compiled and executed by `cargo test`. The `create()` constructor has an illustrative doc example marked ` ```ignore ` — it documents the calling convention but is intentionally NOT compiled or run, because invoking `create()` in a doctest would allocate a real backing file on disk. *(Corrected 2026-08-20 …)* `open_or_create_backing_file`, the interface method implementations, and the lifecycle methods do not currently have individual doc examples — covered by integration tests instead.

**Files**: `specs/001-block-device-filesys/spec.md`

---

## Proposal 2 — FR-023 (BACKFILL-UNSPECCED)

**Direction**: BACKFILL-UNSPECCED (add new requirement to existing spec.md)

**Rationale**: `set_file_path` / `set_block_size` / `set_num_blocks` are crate-private (`pub(crate)`), `#[allow(dead_code)]`, and currently unused (`src/lib.rs:95-109`). They are not public API and produce no observable external behavior, so the drift report suggested they were out of scope for FRs. Per Phase B policy #4, unspecced items are backfilled into the existing spec rather than left undocumented. Because no `.rs` edits are permitted (cannot remove the dead code), the faithful action is to record their presence as intentional and explicitly outside the functional contract — added as **FR-023**, with no acceptance scenario (nothing externally observable to assert). No feature invented beyond what the code shows.

**Before**: *(no requirement — internal setters undocumented)*

**After** (new FR-023):
> The component MAY retain crate-private (`pub(crate)`) configuration mutators `set_file_path`, `set_block_size`, and `set_num_blocks` that overwrite the corresponding config fields after construction. These are reserved internal helpers, are NOT part of the component's public API (the public configuration path is `create(...)` per FR-004/FR-006), and are currently unused (`#[allow(dead_code)]`). … They carry no acceptance scenario and are outside the functional contract … MUST NOT be promoted to public API without a corresponding spec change and validation (values set via `set_block_size`/`set_num_blocks` are only re-validated at `initialize()` via `DeviceConfig::new`, not at set time).

**Files**: `specs/001-block-device-filesys/spec.md`

---

## Not proposed

- **ALIGN**: none. The single drift item is spec-overclaim against intentional code, not a behavioral bug.
- **RESOLVED**: none. No trivial-defect fixes were pending for this component.
- **HUMAN_DECISION**: none. Both items are unambiguous after reading the code.

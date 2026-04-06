# Sync Apply Report

Applied: 2026-04-06T13:00:00Z
Based on: proposals from 2026-04-06T12:30:00Z

## Summary

| Category | Count |
|----------|-------|
| Specs Updated | 4 |
| Code Files Modified | 6 |
| Tests Added | 3 |
| Compile-fail Tests Added | 3 |
| Pre-existing Bug Fixed | 1 |

## Changes Made

### Specs Updated (Backfill)

| Spec | Requirement | Change |
|------|-------------|--------|
| 001-com-component-framework | SC-005 | Reworded: benchmarks must compile; regression detection manual |
| 002-registry-refcount-binding | FR-018 | Reworded: caller holds one ComponentRef; internal count may be higher |
| 003-actor-channels | FR-002 | Reworded: actors MAY implement IUnknown directly when generics require it |
| 005-numa-aware-actors | FR-016 | Reworded: documents first-touch-only design as intentional |
| 005-numa-aware-actors | FR-017 | Reworded: removes framework integration requirement; first-touch is the design |

### Code Changes (Align)

| Proposal | File | Change |
|----------|------|--------|
| P1: validate_cpus | `crates/component-core/src/actor.rs` | Added `validate_cpus()` call before thread spawn in `activate()` |
| P5: IUnknown wiring test | `crates/component-framework/tests/actor.rs` | Added `actor_channel_wiring_via_iunknown_query` test |
| P6: Channel registry test | `crates/component-framework/tests/actor.rs` | Added `channel_registerable_in_component_registry` test |
| P8: register_simple test | `crates/component-framework/tests/registry.rs` | Added `register_simple_creates_component` test |
| P9: Compile-fail tests | `crates/component-framework/src/lib.rs` | Added 3 `compile_fail` doc tests for macro error paths |
| P10: Prelude completeness | `crates/component-core/src/prelude.rs` | Added `Receptacle` re-export; improved doc test |
| P11: CI benchmark gate | `CLAUDE.md` | Added `cargo bench --no-run` to CI gate command |
| P12: Doc test gaps | `crates/component-core/src/component.rs` | Added doc test for `InterfaceMap::info()` |

### Additional Fixes

| File | Change |
|------|--------|
| `crates/component-core/src/actor.rs:321` | Fixed pre-existing doc test: added missing `use component_core::query_interface` import |

## Verification

Full CI gate passed:

- `cargo fmt --check` — clean
- `cargo clippy -- -D warnings` — clean
- `cargo test --all` — 218 unit + 10 NUMA integration + 11 actor integration + 6 binding + 4 registry + 161 doc tests = all pass
- `cargo doc --no-deps` — warning-free
- `cargo bench --no-run` — all benchmarks compile

## Backups

Spec backups stored at: `.specify/sync/backups/2026-04-06/`

## Next Steps

1. Review changes: `git diff`
2. Commit: `git add -A && git commit -m "sync: apply drift resolutions from 2026-04-06 analysis"`

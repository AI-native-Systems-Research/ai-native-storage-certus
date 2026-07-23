# Spec Drift Report

Generated: 2026-07-22T21:28:13Z
Project: dispatch-map

> Note: A prior drift report existed in this file (dated 2026-07-10). It is superseded by this analysis, which found it had drifted itself — e.g. it claimed FR-014's error-logging requirement was satisfied, but no `logger.error(...)` call exists anywhere in the component, and it did not account for `promote_block_to_memory_tier` / `try_evict_to_block`, which are real, consumed public API surface.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (FR/SC + tracked acceptance scenarios) | 30 |
| Aligned | 26 (87%) |
| Drifted | 2 (7%) |
| Not Implemented | 2 (7%) |
| Unspecced Code | 4 |

## Detailed Findings

### Spec: 001-dispatch-map — Dispatch Map Component

#### Aligned

- FR-001 (`CacheKey = u64`) → `components/interfaces/src/idispatch_map.rs:6`
- FR-002 (per-entry metadata: Location, size_blocks, read_ref, write_ref, EvictionHandle, Mutex/Condvar) → `components/dispatch-map/src/entry.rs:9-38`, `components/dispatch-map/src/state.rs:12-21`
- FR-003 (`create_memory_tier_entry` core behavior: MemoryTier location, write_ref=1, AlreadyExists on dup, eviction registration) → `components/dispatch-map/src/lib.rs:367-408`
- FR-004 (`lookup` returns NotExist/BlockDevice/MemoryTier, increments read_ref, blocks on write_ref with 2s default timeout, MismatchSize unused, size not exposed on BlockDevice, eviction priority refreshed on success) → `components/dispatch-map/src/lib.rs:115-158`
- FR-005 (`convert_to_storage` sets `ssd_offset`, conditional read_ref decrement, errors on missing key / already-BlockDevice) → `components/dispatch-map/src/lib.rs:160-192`
- FR-006 (`take_read` blocks on write_ref, 2s timeout) → `components/dispatch-map/src/lib.rs:194-218`
- FR-007 (`take_write` blocks on read_ref/write_ref, 2s timeout) → `components/dispatch-map/src/lib.rs:220-243`
- FR-008 (`release_read` underflow error) → `components/dispatch-map/src/lib.rs:245-263`
- FR-009 (`release_write` underflow error) → `components/dispatch-map/src/lib.rs:265-283`
- FR-010 (`downgrade_reference` atomic write→read, error if no write ref) → `components/dispatch-map/src/lib.rs:285-308`
- FR-011 (`remove` errors on active refs) → `components/dispatch-map/src/lib.rs:310-333`
- FR-012 (`initialize`: eviction_policy mandatory, extent_manager optional, empty map + `Ok(())` when unbound) → `components/dispatch-map/src/lib.rs:68-113`
- FR-013 (thread-safe/re-entrant via `Mutex`/`Condvar`) → `components/dispatch-map/src/state.rs`
- FR-015 (`define_component!` with `IDispatchMap` provided, `ILogger`/`IExtentManager`/`IEvictionPolicy` receptacles) → `components/dispatch-map/src/lib.rs:33-46`
- FR-016 (`touch` via `IEvictionPolicy`, no ref change, `KeyNotFound`) → `components/dispatch-map/src/lib.rs:335-347`
- FR-017 (`oldest_keys` delegates to `IEvictionPolicy::peek_oldest`) → `components/dispatch-map/src/lib.rs:358-365`
- FR-018 (`MemoryTier{pointer,size,ssd_offset}`, `convert_memory_tier_to_block` reads internal `ssd_offset`) → `components/dispatch-map/src/entry.rs:13-19`, `lib.rs:410-449`
- FR-020 (`initialize()` explicit, not called from constructor) → `components/dispatch-map/src/lib.rs:68` (not invoked in `new`/`new_default`)
- FR-022 (`is_evictable` predicate exact match) → `components/dispatch-map/src/lib.rs:499-512`
- FR-023 (`entry_size` = `size_blocks * 4096`, rounds up via `div_ceil`) → `components/dispatch-map/src/lib.rs:349-356,392`
- FR-024 (`recover_extent` inserts BlockDevice entry, `AlreadyExists` guard) → `components/dispatch-map/src/lib.rs:547-571`
- SC-001 (recoverable extents) → tested in `components/dispatch-map/tests/integration.rs:476-522` (`recovery_populated`)
- SC-002 (concurrent readers, no corruption/deadlock) → tested in `tests/integration.rs:42-62,354-384`
- SC-003 (atomic write→read downgrade) → `components/dispatch-map/src/lib.rs:285-308` (single mutex hold)
- SC-005 (lookup non-blocking when no writer active) → `wait_for` predicate short-circuits in `state.rs:43-63`
- SC-006 (ref-count consistency under concurrency) → covered extensively by `tests/integration.rs`

#### Drifted

- **FR-014**: Spec says "System MUST use the `ILogger` receptacle for **info, debug, and error** logging throughout the component."
  Actual: Only `logger.info(...)` and `logger.debug(...)` are called; `logger.error(...)` is never invoked anywhere in the component, despite numerous fallible paths (`KeyNotFound`, `Timeout`, `ActiveReferences`, `RefCountUnderflow`/`Overflow`, `InvalidState`, `InvalidSize`) that return `Err` without logging.
  - Location: `components/dispatch-map/src/lib.rs` (no `logger.error` call sites anywhere; error returns e.g. at lines 130, 165, 203, 232, 250, 270, 290, 315, 340, 415, 458, 519, 558 log nothing)
  - Severity: moderate

- **SC-004**: Spec says "The `DispatchEntry` struct size **varies** by `Location` variant (`BlockDevice` stores a `u64` offset; `MemoryTier` stores a pointer, size, and optional offset)."
  Actual: `Location` is a Rust `enum`; the compiler sizes `DispatchEntry` to fit its largest variant (`MemoryTier`) at compile time. `std::mem::size_of::<DispatchEntry>()` (exposed via the free function `entry_size()` in `lib.rs:16-18`) is a fixed constant — it does not vary per-instance based on which variant is active.
  - Location: `components/dispatch-map/src/entry.rs:9-19,28-38`; `components/dispatch-map/src/lib.rs:16-18`
  - Severity: minor (spec wording is imprecise about Rust enum layout, not a functional defect)

#### Not Implemented

- **User Story 1 / Acceptance Scenario 3 + Edge Cases item 1** — "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded in the map." No null-pointer check exists in `create_memory_tier_entry` (`components/dispatch-map/src/lib.rs:367-408`), and `DispatchMapError` (`components/interfaces/src/idispatch_map.rs:38-62`) has no dedicated null-pointer variant. A null `*mut u8` is currently accepted and stored without error.
- **User Story 2 / Acceptance Scenario 4** — "size mismatch → `ErrorMismatchSize`." `lookup(key)` (`components/interfaces/src/idispatch_map.rs:117`; `lib.rs:115-158`) takes no expected-size parameter, so a caller has no way to trigger this path. FR-004 itself already flags `MismatchSize` as "not currently triggered," so this is a spec-acknowledged (but still open) gap rather than a surprise.

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `promote_block_to_memory_tier(key, pointer, size)` — in-place BlockDevice→MemoryTier promotion preserving refs/eviction handle; consumed by `components/dispatcher/src/lib.rs` and `components/dispatcher-p2p/src/lib.rs` for on-demand cold-block promotion | `components/interfaces/src/idispatch_map.rs:234-239`; `components/dispatch-map/src/lib.rs:451-497` | ~47 | 001-dispatch-map (new FR + user story for the promotion path) |
| `try_evict_to_block(key)` — atomic evictability check + BlockDevice transition under one lock hold; consumed by dispatcher/dispatcher-p2p SSD-evictor paths | `components/interfaces/src/idispatch_map.rs:251-262`; `components/dispatch-map/src/lib.rs:514-545` | ~32 | 001-dispatch-map (new FR alongside FR-022 `is_evictable`) |
| `reuse_count: AtomicU32` field — incremented on `lookup`/`take_read`/`downgrade_reference` but never read or exposed through any `IDispatchMap` method; dead metric | `components/dispatch-map/src/entry.rs:37,49-51`; `components/dispatch-map/src/lib.rs:101,137,213,301,396,567` | ~10 | Wire into a new FR (expose reuse metrics), or remove |
| Formal-verification claims (P1–P10, "24 verification conditions") referencing `components/dispatch-map/verif/`, a directory that does not exist anywhere under this component | `components/interfaces/src/idispatch_map.rs:84-99` | 16 | Add a verification section to spec.md, or correct/remove the stale comment |

## Inter-Spec Conflicts

None — only one spec (`001-dispatch-map`) exists for this component.

## Recommendations

1. Add `logger.error(...)` calls on the component's error-return paths (timeouts, invalid-state transitions, ref-count violations) to satisfy FR-014, or narrow FR-014's wording to match the current info/debug-only logging behavior.
2. Add null-pointer validation to `create_memory_tier_entry` (e.g. a new `DispatchMapError::NullPointer` variant) to close the gap against User Story 1 AS3 and the Edge Cases list, or explicitly drop that acceptance scenario if the check was intentionally omitted as an unsafe-caller contract.
3. Write a new spec increment (e.g. `002-dispatch-map-promotion`) covering `promote_block_to_memory_tier` and `try_evict_to_block` — real, actively consumed public API surface (by `dispatcher` and `dispatcher-p2p`) with zero requirements coverage in the current spec.
4. Decide the fate of `reuse_count`: expose it through a new interface method (e.g. cache-hit telemetry) or remove the dead instrumentation.
5. Correct or remove the Creusot verification-properties comment block in `components/interfaces/src/idispatch_map.rs:84-99` — it references a `verif/` directory that does not exist in this component.
6. `components/dispatch-map/README.md` is stale relative to both the spec and the code: it describes a `Staging`/`DmaBuffer` location variant and RDTSC `tsc` timestamps that no longer exist (current code has only `BlockDevice`/`MemoryTier` and delegates ordering to `IEvictionPolicy`), and never mentions the `IEvictionPolicy` receptacle, `promote_block_to_memory_tier`, `try_evict_to_block`, `is_evictable`, `recover_extent`, or `entry_size`. `components/dispatch-map/CLAUDE.md`'s "Component Wiring" diagram likewise omits the `IEvictionPolicy` receptacle that is present (and mandatory) in the code. Both should be refreshed — see the `component-update-docs` skill.

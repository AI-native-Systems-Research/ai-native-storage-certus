# Spec Drift Report

Generated: 2026-08-06T23:27:50Z
Project: dispatch-map

> Note: Supersedes the prior report (2026-07-22). Since then the spec was
> backfilled with User Stories 10/11 and FR-025 / FR-026, so
> `promote_block_to_memory_tier` and `try_evict_to_block` are now **specced and
> aligned** (previously listed as unspecced code). A new `integrity-check`
> Cargo feature (`set_checksum` / `get_checksum` + a per-entry `checksum` field)
> has since appeared with zero spec coverage.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (FR/SC + tracked acceptance scenarios) | 32 |
| ✓ Aligned | 27 (84%) |
| ⚠️ Drifted | 3 (9%) |
| ✗ Not Implemented | 2 (6%) |
| 🆕 Unspecced Code | 3 |

Active requirements: FR-001–FR-026 (FR-019 removed, FR-021 merged into FR-005 →
24 active FRs) + SC-001–SC-006 + 2 tracked acceptance scenarios.

## Detailed Findings

### Spec: 001-dispatch-map — Dispatch Map Component

#### Aligned ✓

- FR-001 (`CacheKey = u64`) → `components/interfaces/src/idispatch_map.rs:6`
- FR-002 (per-entry metadata: `Location` enum, `size_blocks`, `read_ref`, `write_ref`, `EvictionHandle`, `Mutex`/`Condvar`) → `src/entry.rs:9-43`, `src/state.rs:12-21`
- FR-003 (`create_memory_tier_entry`: MemoryTier, write_ref=1, eviction registration, `AlreadyExists` on dup, `InvalidSize` on 0) → `src/lib.rs:381-424`
- FR-004 (`lookup` → NotExist/BlockDevice/MemoryTier, increments read_ref, blocks on write_ref w/ 2000ms timeout, `MismatchSize` unused, size not exposed on BlockDevice, eviction refreshed on success) → `src/lib.rs:118-164`
- FR-005 (`convert_to_storage` sets `ssd_offset`, conditional read_ref decrement, errors on missing key / already-BlockDevice) → `src/lib.rs:166-198`
- FR-006 (`take_read` blocks on write_ref, 2000ms) → `src/lib.rs:200-227`
- FR-007 (`take_write` blocks on read_ref/write_ref, 2000ms) → `src/lib.rs:229-255`
- FR-008 (`release_read` underflow error) → `src/lib.rs:257-275`
- FR-009 (`release_write` underflow error) → `src/lib.rs:277-295`
- FR-010 (`downgrade_reference` atomic write→read, `NoWriteReference` if none held) → `src/lib.rs:297-322`
- FR-011 (`remove` errors on active refs, `KeyNotFound` if absent) → `src/lib.rs:324-347`
- FR-013 (thread-safe/re-entrant via `Mutex`/`Condvar`) → `src/state.rs`
- FR-015 (`define_component!` with `IDispatchMap` provided; `ILogger`/`IExtentManager`/`IEvictionPolicy` receptacles) → `src/lib.rs:33-46`
- FR-016 (`touch` via `IEvictionPolicy::touch`, no ref change, `KeyNotFound`) → `src/lib.rs:349-361`
- FR-017 (`oldest_keys` delegates to `IEvictionPolicy::get_eviction_candidates(pool, n)`) → `src/lib.rs:372-379`
- FR-018 (`MemoryTier { pointer, size, ssd_offset }`; `convert_memory_tier_to_block` reads internal `ssd_offset`) → `src/entry.rs:13-19`, `src/lib.rs:426-465`
- FR-020 (`initialize()` explicit, not called from constructor) → `src/lib.rs:67` (not invoked in `new`/`new_default`)
- FR-022 (`is_evictable` predicate: MemoryTier + `ssd_offset: Some` + zero refs) → `src/lib.rs:515-531`
- FR-023 (`entry_size` = `size_blocks * 4096`, rounds up via `div_ceil`) → `src/lib.rs:363-370`, `src/lib.rs:406`
- FR-024 (`recover_extent` inserts BlockDevice entry, `AlreadyExists` guard) → `src/lib.rs:566-592`
- FR-025 (`promote_block_to_memory_tier` in place: preserves `EvictionHandle` + all refs, `ssd_offset: Some(orig)`, `KeyNotFound`/`InvalidSize`/`InvalidState`) → `src/lib.rs:467-513` — **newly specced, aligned**
- FR-026 (`try_evict_to_block` atomic evictability check + transition under one lock hold, `KeyNotFound`/`InvalidState`, no partial change) → `src/lib.rs:533-564` — **newly specced, aligned**
- SC-001 (recoverable extents) → `tests/integration.rs` recovery tests
- SC-002 (concurrent readers, no corruption/deadlock) → `tests/integration.rs`
- SC-003 (atomic write→read downgrade, single mutex hold) → `src/lib.rs:297-322`
- SC-005 (lookup non-blocking when no writer) → `wait_for` short-circuits in `src/state.rs:64-66`
- SC-006 (ref-count consistency under concurrency) → `tests/integration.rs`

#### Drifted ⚠️

- **FR-014**: Spec says "System MUST use the `ILogger` receptacle for **info, debug, and error** logging throughout the component."
  - Actual: only `logger.info(...)` and `logger.debug(...)` are called; `logger.error(...)` is never invoked anywhere. Every fallible path (`KeyNotFound`, `Timeout`, `ActiveReferences`, `RefCountUnderflow`/`Overflow`, `InvalidState`, `InvalidSize`, `NoWriteReference`) returns `Err` without logging.
  - Location: `components/dispatch-map/src/lib.rs` (no `logger.error` call sites; error returns e.g. at 129, 178, 208, 264, 284, 304, 331, 388, 396, 444, 497, 541, 557 log nothing)
  - Severity: moderate

- **FR-012**: Spec/contract says `initialize()` "Requires the `IEvictionPolicy` receptacle to be bound (**errors otherwise**)" and "returns an error if unbound".
  - Actual: `initialize()` calls `self.get_pool_id()` first (`src/lib.rs:68`), which does `self.eviction_policy.get().unwrap()` (`src/lib.rs:55`) — this **panics** when the receptacle is unbound rather than returning `DispatchMapError::NotInitialized`. The subsequent `.map_err(...)` guard at `src/lib.rs:69-71` is therefore unreachable in the unbound case.
  - Location: `components/dispatch-map/src/lib.rs:50-59` (`get_pool_id`), `68-71` (`initialize`)
  - Severity: minor (error-contract shape mismatch; panic instead of typed error)

- **SC-004**: Spec says "The `DispatchEntry` struct size **varies** by `Location` variant (`BlockDevice` stores a `u64` offset; `MemoryTier` stores a pointer, size, and optional offset)."
  - Actual: `Location` is a Rust `enum`; the compiler sizes `DispatchEntry` for its largest variant at compile time. `std::mem::size_of::<DispatchEntry>()` (exposed via the free `entry_size()` at `src/lib.rs:16-18`) is a fixed constant — it does not vary per-instance by active variant.
  - Location: `components/dispatch-map/src/entry.rs:9-19`, `28-43`; `src/lib.rs:16-18`
  - Severity: minor (imprecise wording about Rust enum layout, not a functional defect)

#### Not Implemented ✗

- **User Story 1 / Acceptance Scenario 3 + Edge Cases item 1** — "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded in the map." No null-pointer check exists in `create_memory_tier_entry` (`src/lib.rs:381-424`); `DispatchMapError` (`components/interfaces/src/idispatch_map.rs:38-62`) has no null-pointer variant. A null `*mut u8` is accepted and stored.
- **User Story 2 / Acceptance Scenario 4** — "size mismatch → `ErrorMismatchSize`." `lookup(key)` takes no expected-size parameter, so a caller cannot trigger this path. FR-004 itself flags `MismatchSize` as "not currently triggered" — spec-acknowledged but still an open gap.

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `integrity-check` Cargo feature: per-entry `checksum: u32` field + `set_checksum(key, checksum)` / `get_checksum(key) -> Option<u32>` interface methods (CRC-32 that survives demote/promote). spec.md has zero mention of checksum/integrity/CRC. | `components/dispatch-map/Cargo.toml:8-11`; `src/entry.rs:41-42`; `src/lib.rs:594-615`; `components/interfaces/src/idispatch_map.rs` (`set_checksum`/`get_checksum` under feature gate) | ~30 | 001-dispatch-map (new FR + user story for optional integrity checking), or document as an off-by-default experimental feature |
| `reuse_count: AtomicU32` field — incremented on `lookup`/`take_read`/`downgrade_reference` but never read or exposed through any `IDispatchMap` method; dead metric | `src/entry.rs:37,54-55`; `src/lib.rs:142,221,314` | ~8 | Wire into a new FR (expose reuse/hit metrics), or remove |
| Formal-verification claims (P1–P10, "24 verification conditions") referencing `components/dispatch-map/verif/`, a directory that **does not exist** | `components/interfaces/src/idispatch_map.rs:84-99` | 16 | Add a verification section to spec.md and create `verif/`, or correct/remove the stale comment |

## Inter-Spec Conflicts

None — only one spec (`001-dispatch-map`) exists for this component.

## Recommendations

1. **FR-014**: Add `logger.error(...)` calls on error-return paths (timeouts, invalid-state transitions, ref-count violations), or narrow FR-014's wording to match the current info/debug-only behavior.
2. **FR-012**: Replace the `unwrap()` in `get_pool_id` with a fallible path (or reorder `initialize` to check `eviction_policy.get()` before `get_pool_id`) so an unbound `IEvictionPolicy` yields `Err(NotInitialized)` instead of a panic — or amend FR-012/the contract to state that an unbound eviction policy is a caller contract violation (panic).
3. **Null-pointer (US1/AS3)**: Add null-pointer validation to `create_memory_tier_entry` (e.g. a `DispatchMapError::NullPointer` variant), or drop that acceptance scenario as an intentional unsafe-caller contract.
4. **`integrity-check` feature**: Spec the checksum surface (`set_checksum`/`get_checksum`, the `checksum` field, and the feature gate) as an optional FR/user story, or explicitly document it as experimental and out of the v0 spec scope.
5. **`reuse_count`**: Decide its fate — expose via a cache-hit telemetry method (new FR) or remove the dead instrumentation.
6. **Creusot claims**: Correct or remove the P1–P10 verification comment block in `components/interfaces/src/idispatch_map.rs:84-99` — it references a `verif/` directory that does not exist. Either restore the proofs or delete the stale claim.
7. **SC-004**: Reword to describe compile-time enum layout (largest-variant sizing) rather than per-instance variance.

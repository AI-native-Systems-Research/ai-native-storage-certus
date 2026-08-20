# Dispatch-Map — Spec ↔ Implementation Drift Report

**Generated**: pending
**Component**: `components/dispatch-map`

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 32 (26 FR + 6 SC) |
| Aligned | 30 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced | 1 |

Spec analyzed: `001-dispatch-map` — *Dispatch Map Component* (Status: Complete, Last Synced 2026-08-07).

The component is very close to its spec. The two drift items are exactly the two "code-side align" tasks the spec's own `Last Synced` note flagged as still outstanding (`.specify/sync/align-tasks.md`): the FR-012 panic→error conversion and the US1/AS3 null-pointer check. Both remain undone in code.

## Detailed Findings — `001-dispatch-map`

### Aligned ✓

- **FR-001** (`CacheKey = u64`): imported from `interfaces` (`src/lib.rs:26-29`).
- **FR-002** (per-entry metadata: location, size_blocks, read_ref, write_ref, EvictionHandle; Mutex/Condvar): `src/entry.rs:28-43`, `src/state.rs:12-38`.
- **FR-004** (`lookup` returns NotExist/BlockDevice/MemoryTier, increments read_ref, blocks on write_ref with 2000ms timeout, refreshes eviction priority; MismatchSize unused): `src/lib.rs:118-164`, `DEFAULT_TIMEOUT` `src/lib.rs:25`.
- **FR-005** (`convert_to_storage` sets `ssd_offset`, conditional read_ref decrement, errors on absent/BlockDevice): `src/lib.rs:166-198`.
- **FR-006** (`take_read`, waits write_ref==0, 2000ms timeout): `src/lib.rs:200-227`.
- **FR-007** (`take_write`, waits read_ref==0 && write_ref==0): `src/lib.rs:229-255`.
- **FR-008** (`release_read`, underflow error): `src/lib.rs:257-275`.
- **FR-009** (`release_write`, underflow error): `src/lib.rs:277-295`.
- **FR-010** (`downgrade_reference`, NoWriteReference error): `src/lib.rs:297-322`.
- **FR-011** (`remove`, ActiveReferences error): `src/lib.rs:324-347`.
- **FR-013** (thread-safe via Mutex/Condvar): `src/state.rs`, all methods lock `inner`.
- **FR-014** (info/debug logging only; errors returned as typed values, no `logger.error`): confirmed — only `logger.info`/`logger.debug` used (`src/lib.rs:77,84,111,160,...`); no `logger.error` calls in the component.
- **FR-015** (`define_component!` with IDispatchMap provided; ILogger/IExtentManager/IEvictionPolicy receptacles): `src/lib.rs:33-46`.
- **FR-016** (`touch`, delegates to `IEvictionPolicy::touch`, KeyNotFound, no ref changes): `src/lib.rs:349-361`.
- **FR-017** (`oldest_keys` delegates to `IEvictionPolicy::get_eviction_candidates`): `src/lib.rs:372-379`.
- **FR-018** (MemoryTier variant fields + `convert_memory_tier_to_block` reads internal `ssd_offset`): `src/entry.rs:13-19`, `src/lib.rs:426-465`.
- **FR-020** (`initialize` explicit public API; rebuilds via `for_each_extent`; Ok with empty map when no extent manager): `src/lib.rs:67-116`.
- **FR-022** (`is_evictable` — MemoryTier + ssd_offset Some + no refs): `src/lib.rs:515-531`.
- **FR-023** (`entry_size` = size_blocks*4096, KeyNotFound, no ref): `src/lib.rs:363-370`.
- **FR-024** (`recover_extent` inserts BlockDevice, AlreadyExists): `src/lib.rs:566-592`.
- **FR-025** (`promote_block_to_memory_tier` in-place, preserves handle+refs, sets ssd_offset, InvalidSize/KeyNotFound/InvalidState): `src/lib.rs:467-513`.
- **FR-026** (`try_evict_to_block` atomic check+transition under one lock): `src/lib.rs:533-564`.
- **FR-027/FR-028** (`integrity-check` feature: `set_checksum`/`get_checksum`, 0 treated as unset, feature-gated field): `src/lib.rs:594-615`, `src/entry.rs:41-42`.
- **SC-001** (recovery of all committed extents): `initialize` walk `src/lib.rs:90-108`; tested indirectly.
- **SC-002/SC-003/SC-005/SC-006** (concurrency, downgrade atomicity, non-blocking read, ref-count integrity): covered by `src/lib.rs` blocking semantics + tests (`tests/integration.rs`, unit tests `src/lib.rs:642-948`).
- **US2/AS4** (MismatchSize) correctly marked deferred; `lookup` takes no expected-size arg (`src/lib.rs:118`).

### Drifted ⚠️

- **FR-012 — initialize returns error (not panic) if `IEvictionPolicy` unbound** — *moderate*.
  - Spec: "On initialization, the `IEvictionPolicy` receptacle MUST be connected (returns an error if unbound)." The spec's `Last Synced` note lists "FR-012 panic→error" as a pending code-side align.
  - Actual: `initialize` calls `self.get_pool_id()` **first** (`src/lib.rs:68`), and `get_pool_id` does `self.eviction_policy.get().unwrap()` (`src/lib.rs:55`) — an unbound eviction_policy **panics** here before the graceful `map_err(... NotInitialized)` path at `src/lib.rs:69-71` is ever reached. Same unwrap panic pattern in `create_memory_tier_entry` (`src/lib.rs:392`), `recover_extent` (`src/lib.rs:573`), and via `get_pool_id` in `oldest_keys` (`src/lib.rs:373`).
  - Location: `src/lib.rs:55` (and `:68`, `:392`, `:573`).

- **FR-003 / US1-AS3 — null pointer to `create_memory_tier_entry` returns an error** — *moderate*.
  - Spec: US1 Acceptance Scenario 3 and Edge Cases: "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded." Flagged in the `Last Synced` note as a pending code-side align ("US1/AS3 null-pointer check").
  - Actual: `create_memory_tier_entry` validates only `size == 0` → `InvalidSize` (`src/lib.rs:387-389`); there is **no null-pointer check** (`grep -n null src/lib.rs` → none). A null pointer is accepted and an entry is recorded.
  - Location: `src/lib.rs:381-424`.

### Not Implemented ✗

None.

## Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---|---|---|---|
| `reuse_count: AtomicU32` field on `DispatchEntry`, incremented on every `lookup`/`take_read`/`downgrade_reference` | `src/entry.rs`, `src/lib.rs` | entry.rs:37; lib.rs:142-143, 220-222, 313-315 | Either add a requirement + entity field for a per-entry read-hit counter and a getter, or remove the field. The spec's `Last Synced` note already lists `reuse_count` removal as a pending code-side align, and SC-004 / Key Entities describe the entry layout without it. |

## Recommendations

1. **Resolve the two outstanding align tasks** (already tracked in `.specify/sync/align-tasks.md`): move the `eviction_policy` binding check ahead of `get_pool_id()` (or make `get_pool_id` fallible) so `initialize` returns `NotInitialized` instead of panicking (FR-012); add a `pointer.is_null()` guard to `create_memory_tier_entry` returning an error (US1/AS3).
2. **Decide `reuse_count`'s fate**: either remove it (as the sync note intends, restoring the documented entry layout) or spec it explicitly with a query method. It is currently dead-for-callers (no accessor on `IDispatchMap`) and only feeds `Debug`.
3. No path drift: this component references `interfaces` and `component_framework` by crate name, not by the relocated `components/`→`lib/` paths, so the recent crate move introduces no stale-path drift here.

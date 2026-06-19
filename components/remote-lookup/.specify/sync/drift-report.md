# Spec Drift Report

Generated: 2026-06-19
Project: remote-lookup

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 8 |
| ✓ Aligned | 5 (62%) |
| ⚠️ Drifted | 3 (38%) |
| ✗ Not Implemented | 0 (0%) |
| 🆕 Unspecced Code | 2 |

## Detailed Findings

### Spec: 001-remote-lookup-placeholder - Remote Lookup Batch Interface

#### Aligned ✓

- FR-001: `IRemoteLookup` exposes `batch_lookup` with signature `fn batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>` → `components/interfaces/src/iremote_lookup.rs:49-52`
- FR-002: Returns one `Result` per input entry, preserving positional order → `components/remote-lookup/src/lib.rs:45-57` (`.iter().map(...).collect()`)
- FR-004: Placeholder returns `Err(RemoteLookupError::NotFound)` for each entry → `components/remote-lookup/src/lib.rs:55`
- FR-006: Empty slice returns empty Vec (`.iter().map(...).collect()` on empty) → `components/remote-lookup/src/lib.rs:45`
- FR-007: Interface defined in `components/interfaces/src/iremote_lookup.rs` → confirmed

#### Drifted ⚠️

- FR-003: Spec says "When connected, each entry MUST produce a log message" but code always logs (no connection state check — `connect`/`disconnect` removed from interface)
  - Location: `components/remote-lookup/src/lib.rs:48-54`
  - Severity: minor
  - Note: The spec references a "connected" state that no longer exists in the implementation. The interface was simplified to remove connect/disconnect per user direction.

- FR-005: Spec says "When not connected, batch_lookup MUST return NotConnected for every entry" but `NotConnected` variant was removed from `RemoteLookupError` and no connection state exists.
  - Location: `components/interfaces/src/iremote_lookup.rs:10-15`
  - Severity: major
  - Note: The interface was intentionally simplified — `connect`/`disconnect`/`is_connected` removed. `RemoteLookupError::NotConnected` no longer exists.

- FR-008: Spec says "component MUST expose functionality only through IRemoteLookup — no public functions outside the interface" but the component also exposes `join_cluster` and `leave_cluster` which are part of the interface (not a violation of the spirit, but they weren't in the spec).
  - Location: `components/interfaces/src/iremote_lookup.rs:66,80`
  - Severity: minor
  - Note: `join_cluster` and `leave_cluster` were added post-spec as interface methods (not public functions outside the interface), so this is spec-incomplete rather than a violation.

#### Not Implemented ✗

(none)

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `join_cluster` | `components/interfaces/src/iremote_lookup.rs:66` | 8 | 001-remote-lookup-placeholder |
| `leave_cluster` | `components/interfaces/src/iremote_lookup.rs:80` | 8 | 001-remote-lookup-placeholder |

## Inter-Spec Conflicts

None detected.

## Recommendations

1. **Update spec FR-003, FR-005**: Remove references to "connected" state. The interface was simplified — `batch_lookup` always logs and returns `NotFound` regardless of connection state. Replace with: "Each entry MUST produce a log message" (unconditional).
2. **Update spec FR-005**: Remove entirely or replace with: "If `join_cluster` has not been called, `batch_lookup` still functions (placeholder behavior)."
3. **Add FR-009, FR-010**: Document `join_cluster(&str)` and `leave_cluster()` methods added to the interface.
4. **Update Key Entities**: Remove `NotConnected` from `RemoteLookupError` variants list.
5. **Update Acceptance Scenario 3**: Remove the "not connected" scenario or rephrase in terms of cluster membership.

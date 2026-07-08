# Feature Specification: Remote Lookup Component

**Feature Branch**: `001-remote-lookup`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `remote-lookup` component is a placeholder implementation of the `IRemoteLookup` interface within the Certus storage system. It provides the integration point where the dispatcher forwards cache misses (entries not found in memory-tier or SSD cold path) to remote Certus nodes on the network for resolution. Currently, the component is a stub: it logs each request via its `ILogger` receptacle and returns `NotFound` for every lookup entry.

The component follows the Certus COM-inspired component model, using `define_component!` to declare its provided interface (`IRemoteLookup`) and receptacles (`ILogger`). It is designed to be swapped with a real network-transport implementation in the future without requiring changes to upstream consumers (primarily the dispatcher component).

## User Scenarios & Testing

### User Story 1 - Batch Lookup Placeholder (Priority: P1)

As a dispatcher component, I want to forward cache misses to `IRemoteLookup::batch_lookup` so that I can integrate against the remote lookup interface contract without waiting for the actual network transport implementation.

**Acceptance Scenarios**:

1. **Given** the component is instantiated, **When** `batch_lookup` is called with N entries of `(CacheKey, IpcHandle)`, **Then** exactly N results are returned in the same positional order as the input entries.
2. **Given** the component is instantiated with a bound `ILogger`, **When** `batch_lookup` is called, **Then** a log message is emitted for each entry containing the key and buffer size.
3. **Given** the component is instantiated, **When** `batch_lookup` is called with any entries, **Then** each result is `Err(RemoteLookupError::NotFound)`.
4. **Given** the component is instantiated, **When** `batch_lookup` is called with an empty slice, **Then** an empty `Vec` is returned.

### User Story 2 - Cluster Lifecycle Management (Priority: P1)

As a dispatcher component, I want to call `join_cluster` during initialization and `leave_cluster` during shutdown so that the remote lookup component can manage its network connections in the future without changing the dispatcher's call sequence.

**Acceptance Scenarios**:

1. **Given** the component is instantiated, **When** `join_cluster` is called with an endpoint string (e.g., `"192.168.1.10:9090"`), **Then** the call returns `Ok(())` and a log message containing the endpoint is emitted.
2. **Given** the component is instantiated, **When** `leave_cluster` is called, **Then** the call returns `Ok(())` and a log message is emitted.
3. **Given** the `ILogger` receptacle is not bound, **When** `join_cluster` or `leave_cluster` is called, **Then** the call still returns `Ok(())` without panicking.

### User Story 3 - Interface Conformance with IDispatcher (Priority: P2)

As a system integrator, I want `IRemoteLookup::batch_lookup` to accept the same parameter types as `IDispatcher::batch_lookup` (`&[(CacheKey, IpcHandle)]`) so that callers can delegate remote lookups using the same data structures without conversion.

**Acceptance Scenarios**:

1. **Given** a slice of `(CacheKey, IpcHandle)` entries, **When** passed to `IRemoteLookup::batch_lookup`, **Then** the code compiles without type conversion or adapter logic.

### Edge Cases

- Empty input slice returns empty result `Vec`.
- Unbound `ILogger` receptacle: component functions correctly, logging is silently skipped.
- Large batch sizes: component processes all entries without panic (no allocation limit imposed).

## Requirements

### Functional Requirements

- **FR-001**: The component MUST implement the `IRemoteLookup` interface as defined in `components/interfaces/src/iremote_lookup.rs`.
- **FR-002**: `batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>` MUST return one result per input entry, preserving positional order.
- **FR-003**: The placeholder `batch_lookup` implementation MUST return `Err(RemoteLookupError::NotFound)` for every entry (no network I/O).
- **FR-004**: Each entry processed by `batch_lookup` MUST produce a log message via the `ILogger` receptacle containing the cache key and IPC handle size.
- **FR-005**: If the `ILogger` receptacle is not bound, logging MUST be silently skipped without panic or error.
- **FR-006**: When called with an empty slice, `batch_lookup` MUST return an empty `Vec`.
- **FR-007**: `join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>` MUST log the endpoint and return `Ok(())`.
- **FR-008**: `leave_cluster(&self) -> Result<(), RemoteLookupError>` MUST log the call and return `Ok(())`.
- **FR-009**: The component MUST declare `IRemoteLookup` in its `provides` list via `define_component!`.
- **FR-010**: The component MUST declare an `ILogger` receptacle for diagnostic logging.
- **FR-011**: The component MUST expose functionality only through the `IRemoteLookup` interface; no public functions outside the trait implementation.
- **FR-012**: The component MUST support `IUnknown` interface discovery via `query_interface!` macro.

### Non-Functional Requirements

- **NFR-001**: The component MUST compile without warnings under `cargo clippy -- -D warnings`.
- **NFR-002**: Public APIs MUST have doc comments with runnable examples; `cargo doc --no-deps` MUST be warning-free.
- **NFR-003**: The component MUST be `Send + Sync` safe (the `IRemoteLookup` trait object is `Arc<dyn IRemoteLookup + Send + Sync>`).
- **NFR-004**: The component MUST NOT perform any blocking I/O, network operations, or heap allocations beyond the result `Vec`.
- **NFR-005**: The component MUST be usable without SPDK dependencies (part of default workspace members).

## Key Entities

| Entity | Type | Description |
|--------|------|-------------|
| `CacheKey` | `u64` (type alias) | Integer identifier for cached data objects, shared with `IDispatcher` and `IDispatchMap`. |
| `IpcHandle` | `struct { address: *mut u8, size: usize }` | Describes a client memory buffer for DMA transfers (address + size). |
| `RemoteLookupError` | `enum` | Error type with variants `NotFound` (key not on remote) and `TransportError(String)` (network failure). |
| `RemoteLookupComponent` | `struct` | The component implementing `IRemoteLookup`, generated by `define_component!`. |
| `ILogger` | `trait` | Receptacle interface for emitting diagnostic log messages. |

## Dependencies

| Dependency | Type | Purpose |
|------------|------|---------|
| `component-framework` | Workspace crate | Facade re-export of component model macros and traits. |
| `component-core` | Workspace crate | Core traits (`IUnknown`, `query_interface!`). |
| `component-macros` | Workspace crate | Procedural macros (`define_component!`, `define_interface!`). |
| `interfaces` | Workspace crate | Shared interface definitions (`IRemoteLookup`, `ILogger`, `CacheKey`, `IpcHandle`). |

### Integration Points

- **Dispatcher**: Calls `join_cluster` on init, `leave_cluster` on shutdown, and `batch_lookup` for entries not found locally.
- **ILogger**: Receives diagnostic log output from all three interface methods.

## Success Criteria

- **SC-001**: All unit tests pass with `cargo test -p remote-lookup`.
- **SC-002**: Doc tests compile and pass with `cargo test --doc -p remote-lookup`.
- **SC-003**: `cargo clippy -p remote-lookup -- -D warnings` reports zero warnings.
- **SC-004**: `cargo doc -p remote-lookup --no-deps` produces warning-free documentation.
- **SC-005**: The interface compiles with the same `(CacheKey, IpcHandle)` parameter types as `IDispatcher::batch_lookup` without type coercion.
- **SC-006**: The component integrates with the dispatcher via receptacle binding without compile errors.

## Implementation Notes

- The component is version `0.1.0`, signaling pre-release placeholder status.
- The `define_component!` macro auto-generates `IUnknown` implementation, `new()` constructor, and receptacle accessor (`self.logger.get()`).
- The receptacle `get()` returns `Result`, so unbound receptacles yield `Err` which is handled with `if let Ok(logger)` pattern.
- Future iterations will replace the stub `batch_lookup` with actual network transport (likely RDMA or TCP-based) while preserving the interface contract.
- The `TransportError` variant on `RemoteLookupError` is defined but not yet used by the placeholder; it anticipates real network failures.
- Log message format: `"remote-lookup: batch_lookup placeholder - key={key}, size={handle.size}"`.

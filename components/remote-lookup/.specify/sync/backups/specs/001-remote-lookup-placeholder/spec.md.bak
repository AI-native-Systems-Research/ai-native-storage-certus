# Feature Specification: Remote Lookup Batch Interface

**Feature Branch**: `001-remote-lookup-placeholder`

**Created**: 2026-06-19

**Status**: Superseded (2026-07-22) — see [Supersession Notice](#supersession-notice)

## Supersession Notice

> ⚠️ **This spec is superseded by [`002-remote-lookup-rdma`](../002-remote-lookup-rdma/spec.md).**
> The shipped component no longer implements the placeholder behavior described below: it performs
> real network I/O over zyre + one-sided RDMA rather than logging and returning `NotFound`, and
> `IRemoteLookup::batch_lookup` takes `&[(CacheKey, u32 /* size */)]`, not `&[(CacheKey,
> IpcHandle)]` — 002 deliberately drops `IpcHandle` from the `IRemoteLookup` boundary because
> remote-lookup is CPU/DRAM-only (see 002's Clarifications, Q1). That divergence from this spec's
> FR-001/SC-002 is therefore **intentional and resolved by 002**, not drift to fix here. This
> document is retained for history only; do not use it as the current design-of-record — read
> `002-remote-lookup-rdma/spec.md` instead. It was previously mis-stamped `Status: Synced
> (2026-06-19)`; that stamp was incorrect and is corrected here as part of a spec-sync pass
> (see `.specify/sync/drift-report.md`, conflict 1).

**Input**: User description: "The component is a placeholder for performing remote lookups to other nodes running Certus on the same network. The component has an ILogger receptacle. The IRemoteLookup interface should support the batch_lookup function with the same parameters as IDispatch export. For the moment, the lookup implementation should only output some logging statement--this is a placeholder."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Batch Lookup Placeholder (Priority: P1)

A Certus node receives a batch of cache key lookups intended for remote nodes on the network. The system invokes `batch_lookup` on the `IRemoteLookup` interface with a slice of `(CacheKey, IpcHandle)` entries. Since this is a placeholder implementation, the component logs each lookup request via its `ILogger` receptacle and returns a not-found result for each entry.

**Why this priority**: This is the sole deliverable — establishing the interface contract and placeholder behavior so that upstream components (e.g., the dispatcher) can integrate against the remote-lookup interface without waiting for network transport implementation.

**Independent Test**: Can be fully tested by calling `batch_lookup` with sample entries and verifying that (a) log output is produced for each entry and (b) results are returned in the correct order with appropriate error values.

**Acceptance Scenarios**:

1. **Given** the component is instantiated, **When** `batch_lookup` is called with N entries, **Then** N results are returned in the same positional order as the input entries.
2. **Given** the component is instantiated, **When** `batch_lookup` is called, **Then** a log statement is emitted for each entry in the batch via the `ILogger` receptacle.
3. **Given** the component is instantiated, **When** `join_cluster` is called with an endpoint string, **Then** the call returns `Ok(())` and a log message is emitted.

---

### User Story 2 - Interface Conformance with IDispatcher (Priority: P2)

The `batch_lookup` method on `IRemoteLookup` MUST accept the same parameter types as the `batch_lookup` method on `IDispatcher` — specifically `&[(CacheKey, IpcHandle)]` — so that callers can delegate remote lookups using the same data structures without conversion.

**Why this priority**: Type alignment between interfaces ensures zero-cost integration when the real network transport is implemented later.

**Independent Test**: Compilation succeeds with a caller that passes `&[(CacheKey, IpcHandle)]` to both `IDispatcher::batch_lookup` and `IRemoteLookup::batch_lookup` without type coercion.

**Acceptance Scenarios**:

1. **Given** a slice of `(CacheKey, IpcHandle)` entries, **When** passed to `IRemoteLookup::batch_lookup`, **Then** the code compiles without type conversion or adapter logic.

---

### Edge Cases

- What happens when `batch_lookup` is called with an empty slice? Returns an empty `Vec`.
- What happens when the ILogger receptacle is not bound? The component still functions but skips logging silently.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The `IRemoteLookup` interface MUST expose a `batch_lookup` method with signature `fn batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>`.
- **FR-002**: The `batch_lookup` method MUST return one `Result` per input entry, preserving positional order.
- **FR-003**: Each entry in `batch_lookup` MUST produce a log message via the `ILogger` receptacle (placeholder behavior). If the ILogger receptacle is not bound, logging is silently skipped.
- **FR-004**: When connected, the placeholder implementation MUST return `Err(RemoteLookupError::NotFound)` for each entry (no actual network I/O).
- **FR-005**: The `batch_lookup` method MUST be callable at any time after component instantiation. No connection precondition is required.
- **FR-006**: When called with an empty slice, `batch_lookup` MUST return an empty `Vec`.
- **FR-007**: The interface definition MUST reside in `components/interfaces/src/iremote_lookup.rs`.
- **FR-008**: The component MUST expose functionality only through the `IRemoteLookup` interface — no public functions outside the interface.
- **FR-009**: The `IRemoteLookup` interface MUST expose a `join_cluster` method with signature `fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>`. The placeholder implementation MUST log the endpoint and return `Ok(())`.
- **FR-010**: The `IRemoteLookup` interface MUST expose a `leave_cluster` method with signature `fn leave_cluster(&self) -> Result<(), RemoteLookupError>`. The placeholder implementation MUST log the call and return `Ok(())`.

### Key Entities

- **CacheKey**: Integer type alias used as the lookup key (same as used in `IDispatcher`).
- **IpcHandle**: Opaque handle describing client GPU memory for DMA transfers (address + size).
- **RemoteLookupError**: Error enum covering `NotFound` and `TransportError` variants.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All unit tests for `batch_lookup` pass with `cargo test -p remote-lookup`.
- **SC-002**: The interface compiles with the same `(CacheKey, IpcHandle)` parameter types as `IDispatcher::batch_lookup`.
- **SC-003**: Documentation tests for `batch_lookup` compile and pass under `cargo test --doc -p remote-lookup`.
- **SC-004**: `cargo clippy -- -D warnings` reports zero warnings for the component.
- **SC-005**: `cargo doc --no-deps` produces warning-free documentation for the public API.

## Assumptions

- The `IpcHandle` type from `interfaces` is available without the `spdk` feature gate for the purpose of defining the interface signature.
- The placeholder implementation does not perform any network I/O — it is purely a stub that logs and returns errors.
- The `CacheKey` type is the same type alias used in `IDispatcher` (imported from `interfaces`).
- The component already exists with `ILogger` receptacle and `IRemoteLookup` interface wiring — this spec adds `batch_lookup` to the existing interface.

# Research: Remote Lookup Batch Interface

## R1: Type Availability Without SPDK Feature Gate

**Question**: Can `CacheKey` and `IpcHandle` be used in `IRemoteLookup` without enabling the `spdk` feature?

**Decision**: Yes — both types are exported unconditionally from `components/interfaces/src/lib.rs`.

**Rationale**: `CacheKey` is a `pub type CacheKey = u64` alias in `idispatch_map.rs` (line 6, no feature gate). `IpcHandle` is exported at `lib.rs:26` without `#[cfg(feature = "spdk")]`. The `IDispatcher` trait itself is gated behind `spdk`, but its parameter types are not.

**Alternatives considered**: Defining separate types for the remote interface — rejected because it would break the type-alignment requirement with `IDispatcher::batch_lookup`.

## R2: Return Type for batch_lookup

**Question**: Should `batch_lookup` return `Vec<Result<(), DispatcherError>>` (matching IDispatcher) or `Vec<Result<(), RemoteLookupError>>` (using the component's own error type)?

**Decision**: Use `Vec<Result<(), RemoteLookupError>>` — the component's own error type.

**Rationale**: `IRemoteLookup` has its own `RemoteLookupError` enum which covers the relevant error cases (`NotConnected`, `NotFound`, `TransportError`). Using `DispatcherError` would create a dependency on dispatcher-specific error semantics (e.g., `AllocationFailed`, `Timeout`) that don't apply to remote lookups. The caller (dispatcher) can map `RemoteLookupError` variants to `DispatcherError` at the call site.

**Alternatives considered**: Using `DispatcherError` directly — rejected because it couples the remote-lookup component to dispatcher internals and exports error variants that have no meaning in a remote context.

## R3: IpcHandle Ownership in batch_lookup Signature

**Question**: Should `batch_lookup` take `&[(CacheKey, IpcHandle)]` (borrowing) or `Vec<(CacheKey, IpcHandle)>` (owning)?

**Decision**: Use `&[(CacheKey, IpcHandle)]` — borrowed slice, matching `IDispatcher::batch_lookup` exactly.

**Rationale**: The spec requires "same parameters as IDispatch export." `IDispatcher::batch_lookup` takes `&[(CacheKey, IpcHandle)]`. Using a borrowed slice avoids allocation and lets the caller retain ownership of the IPC handles.

**Alternatives considered**: Taking ownership via `Vec` — rejected because it doesn't match IDispatcher and forces unnecessary allocation.

## R4: Placeholder Logging Behavior

**Question**: What should the log message contain for each entry?

**Decision**: Log the `CacheKey` value and the `IpcHandle` size for each entry. Format: `"remote-lookup: batch_lookup placeholder - key={key}, size={size}"`.

**Rationale**: Logging the key and size provides enough information for debugging and integration testing without exposing raw memory addresses. The IpcHandle address is a GPU pointer that has no diagnostic value in logs.

**Alternatives considered**: Logging only the batch size (one message per call) — rejected because the spec says "a log statement is emitted for each entry."

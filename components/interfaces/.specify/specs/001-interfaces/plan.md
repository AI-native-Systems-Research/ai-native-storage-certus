# Implementation Plan: Interfaces

**Branch**: `001-interfaces` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The `interfaces` crate is the centralized contract layer for the Certus component system. It defines 14 interface traits and approximately 40 supporting types (structs, enums, type aliases) that all components depend on instead of depending on each other directly. The crate uses Cargo feature gates (`spdk`, `gpu`) to partition hardware-dependent interfaces from always-available ones, enabling compilation on any Linux system without specialized hardware.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-core` (workspace) -- Core framework traits (`IUnknown`), channel types (`Sender`, `Receiver`), NUMA utilities
- `component-macros` (workspace) -- `define_interface!` proc macro for trait generation with automatic `IUnknown` integration
- `spdk-sys` (workspace, optional) -- Raw FFI bindings to SPDK C libraries, gated behind `features = ["spdk"]`

## Architecture

### Component Layer

```
+-------------------------------------------------------------+
|                    Consuming Components                       |
|  (dispatcher, dispatch-map, memory-tier, block-device, ...)  |
+-------------------------------------------------------------+
                           |
                    depends on (traits only)
                           |
                           v
+-------------------------------------------------------------+
|                    interfaces crate                           |
|                                                              |
|  +-------------------------------------------------------+  |
|  | Always Available (no feature gates)                    |  |
|  |   IGreeter, ILogger, IEvictionPolicy, IGpuServices,   |  |
|  |   IRemoteLookup, IRemoteRequestHandler,               |  |
|  |   IExtendedMetadataStore                              |  |
|  |   + CacheKey, PoolId, EvictionHandle, GpuDeviceInfo,  |  |
|  |     GpuIpcHandle, GpuDmaBuffer, GpuStream, IpcHandle, |  |
|  |     LookupRef, DispatcherConfig, DispatcherError,     |  |
|  |     DispatchMapError, MemoryTierError,                |  |
|  |     ExtentManagerError, FormatParams, WriteHandle,     |  |
|  |     Extent, ExtentKey, PartitionInfo, PartitionSpec,  |  |
|  |     PartitionConfig, PartitionTable,                  |  |
|  |     PartitionTableError, type_guids                   |  |
|  +-------------------------------------------------------+  |
|  | Feature: spdk                                         |  |
|  |   ISPDKEnv, IBlockDevice, IBlockDeviceAdmin,          |  |
|  |   IDispatcher, IDispatchMap, IMemoryTier,             |  |
|  |   IExtentManager, IPartitionTable                     |  |
|  |   + DmaBuffer, PciAddress, PciId, VfioDevice,         |  |
|  |     BlockDeviceError, SpdkEnvError, DmaAllocFn,       |  |
|  |     Command, Completion, ClientChannels, OpHandle,     |  |
|  |     NamespaceInfo, TelemetrySnapshot, NvmeBlockError,  |  |
|  |     LookupResult                                      |  |
|  +-------------------------------------------------------+  |
|  | Feature: gpu (associated types within IGpuServices)   |  |
|  +-------------------------------------------------------+  |
+-------------------------------------------------------------+
                           |
                    depends on (proc macros + core traits)
                           |
                           v
+-------------------------------------------------------------+
|  component-core          |       component-macros            |
|  (IUnknown, channels)    |  (define_interface! proc macro)   |
+-------------------------------------------------------------+
```

### Internal Module Structure

```
components/interfaces/
  Cargo.toml
  src/
    lib.rs                        -- Crate root: module declarations, re-exports, feature gates
    igreeter.rs                   -- IGreeter trait (7 LOC)
    ilogger.rs                    -- ILogger trait (10 LOC)
    ieviction_policy.rs           -- IEvictionPolicy + EvictionHandle, PoolId, EvictionPolicyError (105 LOC)
    igpu_services.rs              -- IGpuServices + GpuDeviceInfo, GpuIpcHandle, GpuDmaBuffer, GpuStream (737 LOC)
    iremote_lookup.rs             -- IRemoteLookup + RemoteLookupError (95 LOC)
    iremote_request_handler.rs    -- IRemoteRequestHandler + LookupRef, RemoteRequestHandlerError (153 LOC)
    iextended_metadata_store.rs   -- IExtendedMetadataStore + ExtendedMetadataStoreError (47 LOC)
    idispatch_map.rs              -- IDispatchMap + CacheKey, LookupResult, DispatchMapError (244 LOC)
    idispatcher.rs                -- IDispatcher + DispatcherConfig, IpcHandle, DispatcherError (665 LOC)
    imemory_tier.rs               -- IMemoryTier + MemoryTierError (212 LOC)
    iextent_manager.rs            -- IExtentManager + Extent, ExtentKey, FormatParams, WriteHandle, ExtentManagerError (262 LOC)
    ispdk_env.rs                  -- ISPDKEnv trait (27 LOC)
    iblock_device.rs              -- IBlockDevice, IBlockDeviceAdmin + Command, Completion, ClientChannels, NvmeBlockError, TelemetrySnapshot, OpHandle, NamespaceInfo (559 LOC)
    ipartition_table.rs           -- IPartitionTable + PartitionInfo, PartitionSpec, PartitionConfig, PartitionTable, PartitionTableError, type_guids (129 LOC)
    spdk_types.rs                 -- DmaBuffer, PciAddress, PciId, VfioDevice, BlockDeviceError, SpdkEnvError, DmaAllocFn, set/is_spdk_env_active (419 LOC)
```

Total: ~3,766 lines of Rust across 16 source files.

### Data Flow

The `interfaces` crate is purely declarative -- it defines no runtime behavior. Data flows through it at compile time:

1. **Trait definition**: Each interface file uses `define_interface!` to generate a trait extending `IUnknown`.
2. **Type sharing**: Error enums, config structs, and handle types are defined alongside their trait, then re-exported from `lib.rs`.
3. **Feature gating**: `lib.rs` conditionally declares modules (`#[cfg(feature = "spdk")]`) and re-exports, partitioning the public API.
4. **Cross-module references**: Several modules reference types from siblings (e.g., `idispatcher.rs` imports `CacheKey` from `idispatch_map.rs`, `igpu_services.rs` references `DmaBuffer` from `spdk_types.rs`).

At runtime, components implement these traits and wire together via the component framework's receptacle binding, using `query_interface()` for dynamic dispatch.

### Key Design Decisions

1. **Single crate for all interfaces**: Avoids circular dependency issues that would arise if each component crate defined its own interface. All cross-component communication types live here.

2. **Feature-gated hardware dependencies**: The `spdk` feature controls 8 interface traits and ~20 associated types. Without it, the crate compiles on any Linux system. The `gpu` feature gates GPU-specific associated types within `IGpuServices`.

3. **Error types always available**: Even SPDK-related error enums (`DispatcherError`, `DispatchMapError`, `MemoryTierError`, etc.) are exported without feature gates. This allows consuming components to handle errors generically.

4. **Config/param structs always available**: `DispatcherConfig`, `FormatParams`, `PartitionConfig` etc. are available without SPDK, enabling configuration logic to compile independently of hardware.

5. **`define_interface!` macro**: Every trait uses this macro to automatically extend `IUnknown`, enabling runtime interface discovery via `query_interface()`. This is the COM-inspired pattern.

6. **Explicit `unsafe impl Send/Sync`**: Types containing raw pointers (`IpcHandle`, `LookupResult::MemoryTier`, `DmaBuffer`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`, `LookupRef`, `Command`, `Completion`) have explicit `Send`/`Sync` impls with `// SAFETY:` justification.

7. **`WriteHandle` two-phase commit**: Uses `Option<Box<dyn FnOnce()>>` closures for publish/abort, enforcing exactly-once semantics via Rust's move semantics. Auto-aborts on drop.

8. **`DmaBuffer` conditional deallocation**: The `Drop` impl checks `is_spdk_env_active()` before calling the C deallocator, preventing segfaults after SPDK teardown.

9. **Formally verified properties**: Each interface file documents Creusot-verified invariants (P1..P10) and unchecked properties with suggested verification techniques, totaling 50+ properties across the crate.

10. **`&self` receivers**: All trait methods use `&self` to support concurrent access through `Arc` wrappers, consistent with the actor-model concurrency pattern.

## Dependencies

| Dependency | Type | Purpose |
|------------|------|---------|
| `component-core` | workspace, always | `IUnknown` trait, `Sender`/`Receiver` channel types |
| `component-macros` | workspace, always | `define_interface!` proc macro |
| `spdk-sys` | workspace, optional | FFI bindings used by `DmaBuffer::new` for `spdk_dma_zmalloc`/`spdk_dma_free` |

## Testing

The crate contains unit tests in 7 modules (inline `#[cfg(test)]` blocks):

- **`ieviction_policy`**: Handle accessor tests, error display formatting
- **`idispatcher`**: Error display for all 7 `DispatcherError` variants, config clone, IpcHandle creation
- **`iblock_device`**: OpHandle equality/hash, NamespaceInfo clone, NvmeBlockError display, `From` conversions, telemetry clone, Command/Completion construction
- **`imemory_tier`**: Error display for all 7 `MemoryTierError` variants
- **`iremote_lookup`**: Error display for both `RemoteLookupError` variants
- **`iremote_request_handler`**: Error display for all 4 variants, `Send+Sync` static assertion for `LookupRef`

**Test command**: `cargo test -p interfaces` (runs without SPDK since tests don't exercise SPDK-gated code paths).

## Future Considerations

1. **Typed error unification**: Consider a common error trait or `thiserror` derive to reduce boilerplate across 9 error enums.
2. **Interface versioning**: As the system evolves, interface version negotiation may be needed (currently all interfaces are v1).
3. **Async trait support**: Current traits are synchronous with `&self`. When Rust stabilizes async-in-traits, high-latency operations (network, GPU DMA) could benefit from `async fn`.
4. **GPU feature expansion**: The `gpu` feature currently only gates associated types within `IGpuServices`. A dedicated `ICudaStream` or `IGpuMemory` interface may emerge as GPU operations grow more complex.
5. **Documentation coverage**: Several interface files (notably `ispdk_env.rs`, `iextended_metadata_store.rs`) lack doc examples on trait methods.
6. **Property-based testing**: The spec identifies numerous "unchecked" properties. PropTest or similar frameworks could systematically validate ordering/consistency guarantees.

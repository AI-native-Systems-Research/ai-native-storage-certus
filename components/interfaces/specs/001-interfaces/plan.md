# Implementation Plan: Shared Interface Trait Definitions

**Branch**: `001-interfaces` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.

## Summary

The `interfaces` crate is fully implemented and serves as the centralized definition point for all Certus component interface traits. It defines 20 `define_interface!` interface traits (180 methods), their supporting types, and feature gates (`spdk`, `integrity-check`, `gpu`) that allow builds without hardware dependencies. This plan documents the existing architecture for maintenance and evolution purposes.

## Technical Context

### Crate Structure

```
components/interfaces/
  Cargo.toml           # Features: default=[], integrity-check=[], spdk=["dep:spdk-sys"], gpu=[]
  src/
    lib.rs                           # Module declarations, conditional compilation, re-exports
    igreeter.rs                      # IGreeter (1 method)
    ilogger.rs                       # ILogger (4 methods)
    ispdk_env.rs                     # ISPDKEnv (5 methods); module spdk-gated
    iblock_device.rs                 # IBlockDevice (11), IBlockDeviceAdmin (6), + types; module spdk-gated
    ieviction_policy.rs              # IEvictionPolicy (9 methods), + types
    idispatch_map.rs                 # IDispatchMap (20 methods, 2 of them integrity-check-gated), + types
    idispatcher.rs                   # IDispatcher (25 methods, trait spdk-gated), + ungated types
    imemory_tier.rs                  # IMemoryTier (21 methods), + bounce-reserve types
    iextent_manager.rs               # IExtentManager (14 methods, trait spdk-gated), + ungated types
    igpu_services.rs                 # IGpuServices (25 methods, 11 of them spdk-gated), + types
    iremote_lookup.rs                # IRemoteLookup (5 methods), + types
    iremote_lookup_rdma_initiator.rs # IRemoteLookupRdmaInitiator (6 methods), + RemoteRegion, PushStatus
    iremote_lookup_rdma_responder.rs # IRemoteLookupRdmaResponder (3), IRemoteLookupRdmaResponderAdmin (5), + types
    itransfer_endpoint.rs            # ITransferEndpoint (6), ITransferEndpointAdmin (3), + types
    izyre.rs                         # IZyre (2 methods), + the plain IZyreNode trait and types
    iextended_metadata_store.rs      # IExtendedMetadataStore (5 methods), + error type
    ipartition_table.rs              # IPartitionTable (4 methods, trait spdk-gated), + ungated types
    spdk_types.rs                    # DmaBuffer, PciAddress, PciId, VfioDevice, error types; spdk-gated
```

### Interface Count

Counted from the `define_interface!` blocks in `src/`. `IZyreNode` is a plain trait, not a component interface, and is not counted. *(Recounted 2026-10-30: `IGpuServices::memcpy_batch_async` (`4941320a`, spdk-gated) and `IDispatcher::clear_all` (added on unstable in `266ed975`; this table had not been updated for it) bring the total from 178 to 180.)*

| Interface | Methods | Feature Gate |
|-----------|---------|--------------|
| IGreeter | 1 | none |
| ILogger | 4 | none |
| ISPDKEnv | 5 | spdk |
| IBlockDevice | 11 | spdk |
| IBlockDeviceAdmin | 6 | spdk |
| IEvictionPolicy | 9 | none |
| IDispatchMap | 20 | none (`set_checksum`/`get_checksum`: integrity-check) |
| IDispatcher | 25 | spdk |
| IMemoryTier | 21 | none |
| IExtentManager | 14 | spdk |
| IGpuServices | 25 | none (11 DMA methods spdk-gated) |
| IRemoteLookup | 5 | none |
| IRemoteLookupRdmaInitiator | 6 | none |
| IRemoteLookupRdmaResponder | 3 | none |
| IRemoteLookupRdmaResponderAdmin | 5 | none |
| ITransferEndpoint | 6 | none |
| ITransferEndpointAdmin | 3 | none |
| IZyre | 2 | none |
| IExtendedMetadataStore | 5 | none |
| IPartitionTable | 4 | spdk |
| **Total** | **180** | |

## Architecture

### Design Principles

1. **Single Definition Point**: All interface traits live in one crate to prevent circular dependencies.
2. **Feature-Gated Compilation**: SPDK-dependent interfaces only compile when `--features spdk` is enabled.
3. **Macro-Generated Traits**: `define_interface!` ensures every interface gets `IUnknown` as a supertrait for runtime discovery.
4. **Types Co-Located With Interfaces**: Error types, handles, and configs are defined alongside their interface traits.
5. **No Implementation Code**: This crate contains zero implementation logic, only type definitions and trait declarations.

### Dependency Direction

```
implementations (block-device-spdk-nvme, dispatcher, etc.)
        |
        v
  [interfaces]  <-- all components depend on this
        |
        v
  [component-core + component-macros]
```

### Conditional Compilation Strategy

- Module-level `#[cfg(feature = "spdk")]` for entire files (`ispdk_env.rs`, `spdk_types.rs`, `iblock_device.rs`).
- Item-level `#[cfg(feature = "spdk")]` on the `IDispatcher`, `IExtentManager` and `IPartitionTable` traits and their re-exports in `lib.rs`; their supporting types stay ungated.
- Method-level `#[cfg(feature = "spdk")]` within `IGpuServices` for DMA operations requiring `DmaBuffer`, and `#[cfg(feature = "integrity-check")]` on `IDispatchMap::set_checksum`/`get_checksum`.

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `component-core` | workspace | IUnknown, channels, NUMA types |
| `component-macros` | workspace | `define_interface!` proc macro |
| `spdk-sys` | workspace (optional) | FFI for DMA allocation in `DmaBuffer::new` |

## Testing

### Current Test Coverage

- Unit tests in `iblock_device.rs`: OpHandle equality/hash, NamespaceInfo clone, NvmeBlockError display/conversions, TelemetrySnapshot clone, Command/Completion matching.
- Unit tests in `idispatcher.rs`: DispatcherError display variants, DispatcherConfig clone, IpcHandle creation.
- Unit tests in `ieviction_policy.rs`: EvictionHandle accessors, EvictionPolicyError display.
- Unit tests in `imemory_tier.rs`: MemoryTierError display variants.
- Unit tests in `iremote_lookup.rs`: RemoteLookupError display.
- Unit tests in `iremote_lookup_rdma_initiator.rs` (error display, PushStatus, RemoteRegion) and `izyre.rs` (ZyreError display, PeerId, ZyreEvent).
- Doctests on the public types and traits, including `itransfer_endpoint.rs` and the bounce-reserve types in `imemory_tier.rs`.

### Test Command

```bash
cargo test -p interfaces           # Default features
cargo test -p interfaces --features spdk  # With SPDK types
```

## Future Considerations

1. **Interface Versioning**: As the system evolves, interfaces may need versioned variants (e.g., `IBlockDevice2`) without breaking existing components.
2. **Async Trait Support**: Currently all traits use synchronous methods. When Rust stabilizes `async fn in traits`, consider async variants for I/O-bound operations.
3. **Error Consolidation**: Some error types share patterns (e.g., `NotInitialized`, `KeyNotFound`). A generic error framework could reduce boilerplate.
4. **GPU Feature Gate**: The `gpu` feature is defined but not yet used for conditional compilation of GPU-specific methods (they currently compile unconditionally outside of SPDK-gated methods).
5. **Formal Verification Coverage**: Five interfaces have formally verified properties (50+ verification conditions). Extending this to remaining interfaces would strengthen correctness guarantees.

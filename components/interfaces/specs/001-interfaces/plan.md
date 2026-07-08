# Implementation Plan: Shared Interface Trait Definitions

**Branch**: `001-interfaces` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.

## Summary

The `interfaces` crate is fully implemented and serves as the centralized definition point for all Certus component interface traits. It defines 15 interface traits, 26+ supporting types, and uses feature gates (`spdk`, `gpu`) to allow builds without hardware dependencies. This plan documents the existing architecture for maintenance and evolution purposes.

## Technical Context

### Crate Structure

```
components/interfaces/
  Cargo.toml           # Features: default=[], spdk=["dep:spdk-sys"], gpu=[]
  src/
    lib.rs             # Module declarations, conditional compilation, re-exports
    igreeter.rs        # IGreeter trait (1 method)
    ilogger.rs         # ILogger trait (4 methods)
    ispdk_env.rs       # ISPDKEnv trait (5 methods, spdk-gated)
    iblock_device.rs   # IBlockDevice (10 methods), IBlockDeviceAdmin (6 methods), + types
    ieviction_policy.rs # IEvictionPolicy trait (9 methods)
    idispatch_map.rs   # IDispatchMap trait (16 methods, spdk-gated), + types
    idispatcher.rs     # IDispatcher trait (16 methods, spdk-gated), + types
    imemory_tier.rs    # IMemoryTier trait (16 methods, spdk-gated)
    iextent_manager.rs # IExtentManager trait (14 methods, spdk-gated), + types
    igpu_services.rs   # IGpuServices trait (22 methods), + types
    iremote_lookup.rs  # IRemoteLookup trait (3 methods)
    iremote_request_handler.rs # IRemoteRequestHandler trait (4 methods), + types
    iextended_metadata_store.rs # IExtendedMetadataStore trait (5 methods)
    ipartition_table.rs # IPartitionTable trait (4 methods), + types
    spdk_types.rs      # DmaBuffer, PciAddress, PciId, VfioDevice, error types
```

### Interface Count

| Interface | Methods | Feature Gate |
|-----------|---------|--------------|
| IGreeter | 1 | none |
| ILogger | 4 | none |
| ISPDKEnv | 5 | spdk |
| IBlockDevice | 10 | spdk |
| IBlockDeviceAdmin | 6 | spdk |
| IEvictionPolicy | 9 | none |
| IDispatchMap | 16 | spdk |
| IDispatcher | 16 | spdk |
| IMemoryTier | 16 | spdk |
| IExtentManager | 14 | spdk |
| IGpuServices | 22 | none (some methods spdk-gated) |
| IRemoteLookup | 3 | none |
| IRemoteRequestHandler | 4 | none |
| IExtendedMetadataStore | 5 | none |
| IPartitionTable | 4 | spdk |
| **Total** | **135** | |

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
- Item-level `#[cfg(feature = "spdk")]` for individual re-exports in `lib.rs`.
- Method-level `#[cfg(feature = "spdk")]` within `IGpuServices` for DMA operations requiring `DmaBuffer`.

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
- Unit tests in `iremote_request_handler.rs`: Error display variants, LookupRef Send+Sync bounds.

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

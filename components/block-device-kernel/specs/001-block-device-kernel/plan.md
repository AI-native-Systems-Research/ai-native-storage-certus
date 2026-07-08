# Implementation Plan: Block Device Kernel Component

**Branch**: `001-block-device-kernel` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)

**Context**: Backfilled from existing implementation.

## Summary

A kernel-native block device component that implements the `IBlockDevice` interface using a raw Linux block device (`/dev/nvme*`) as the backing store. All IO is performed exclusively through `io_uring` with `O_DIRECT | O_DSYNC` semantics — no pread/pwrite fallback exists. The component uses the Certus actor model with a dedicated thread running an io_uring event loop, processing commands from multiple clients via lock-free SPSC channels.

This component serves as a kernel-native alternative to `block-device-spdk-nvme`, providing identical interface semantics without requiring SPDK userspace drivers. It targets environments where kernel-mediated IO is preferred over full userspace NVMe bypass.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**: component-core, component-macros, component-framework, interfaces (with `spdk` feature), io-uring 0.7, libc 0.2, crossbeam-channel 0.5

**Storage**: Raw Linux block device (e.g., `/dev/nvme0n1`), opened with `O_DIRECT | O_DSYNC`

**Testing**: `cargo test -p block-device-kernel` for unit tests; integration tests require real hardware (marked `#[ignore]`); `cargo bench` for Criterion benchmarks

**Target Platform**: Linux x86_64 (kernel >= 5.1 for io_uring, tested on RHEL 9 kernel 5.14)

**Project Type**: Library (Rust component crate)

**Performance Goals**: Near-native NVMe latency for 4KB sync IO; throughput scaling with multi-block transfers

**Constraints**: Requires raw block device access (root/disk group), O_DIRECT mandatory (no buffered IO fallback), single io_uring ring per actor

**Scale/Scope**: ~820 SLOC across 4 source files, 1 integration test file, 2 benchmark files

## Architecture

### Module Structure

```text
src/
├── lib.rs               # Component definition (define_component!), IBlockDevice + IBlockDeviceAdmin impl
├── actor.rs             # KernelHandler — io_uring event loop, command dispatch, completion harvesting
├── config.rs            # DeviceConfig validation, block device open (O_DIRECT|O_DSYNC), BLKGETSIZE64
└── telemetry.rs         # Feature-gated TelemetryStats (atomic counters, snapshot generation)

benches/
├── latency.rs           # Criterion: command construction + sync 4KB IO latency
└── throughput.rs        # Criterion: write/read throughput at 1/8/32/128 × 4KB blocks

tests/
└── integration.rs       # Hardware-dependent tests (write/read roundtrip, async, write-zeros, errors)
```

### Data Flow

```
Client Thread                    Actor Thread
─────────────                    ────────────
Command ──→ [SPSC ingress_tx] ──→ [ingress_rx] → process_command()
                                                        │
                                                        ├─ Sync: submit SQE → submit_and_wait(1) → wait_for_cqe()
                                                        │                                              │
                                                        ├─ Async: submit SQE → insert inflight map     │
                                                        │                                              │
                                                        └─ on_idle(): poll_clients() → harvest_completions() → check_timeouts()
                                                                                              │
Completion ←─ [SPSC callback_rx] ←─ [callback_tx] ←── send_completion() ←────────────────────┘
```

### Key Design Decisions

1. **io_uring only**: No pread/pwrite fallback simplifies the code path and ensures consistent latency characteristics.
2. **O_DSYNC for durability**: Per-write durability without explicit fsync — the kernel guarantees data + metadata are on media before write returns.
3. **O_DIRECT for cache bypass**: Prevents double-buffering; DmaBuffer contents go directly to device via DMA.
4. **Single ring, single thread**: One io_uring instance per actor avoids cross-thread synchronization on the ring.
5. **posix_fadvise on init**: Drops stale page-cache pages from prior buffered sessions to prevent stale reads.
6. **Actor stays alive while clients connected**: `on_idle()` returns `true` as long as `clients` or `inflight` maps are non-empty.

## Dependencies

### Component Dependencies (Workspace)

| Component | Role |
|-----------|------|
| `component-core` | Actor framework, SPSC channels, IUnknown |
| `component-macros` | `define_component!` proc macro |
| `component-framework` | Facade re-export |
| `interfaces` | IBlockDevice, IBlockDeviceAdmin, Command, Completion, DmaBuffer, error types |

### External Crates

| Crate | Version | Role |
|-------|---------|------|
| `io-uring` | 0.7 | io_uring SQ/CQ ring management |
| `libc` | 0.2 | posix_memalign, stat, ioctl, fcntl, posix_fadvise, close, free |
| `crossbeam-channel` | 0.5 | Available for MPSC (currently unused; channels via component-core) |

### System Requirements

- Linux kernel >= 5.1 (io_uring syscalls)
- Raw block device with appropriate permissions
- `O_DIRECT` support on the block device (all real block devices support this)
- Sufficient `memlock` limits for io_uring if using large ring depths

## Testing

### Unit Tests (in `src/lib.rs`)

| Test | Validates |
|------|-----------|
| `component_version` | Version string "0.1.0" |
| `component_provides_iblock_device` | Interface discovery via IUnknown |
| `component_has_logger_receptacle` | Receptacle declaration |
| `connect_client_not_initialized` | Error before initialize() |
| `device_info_returns_configured_values` | block_size, max_queue_depth, num_io_queues, max_transfer_size, numa_node, nvme_version |
| `sector_size_invalid_namespace` | ns_id != 1 rejection |
| `num_sectors_valid` | Configured value returned |
| `telemetry_not_available_without_feature` | FeatureNotEnabled error |

### Unit Tests (in `src/config.rs`)

| Test | Validates |
|------|-----------|
| `block_size_too_small` | Rejects < 512 |
| `block_size_not_power_of_two` | Rejects non-power-of-2 |
| `rejects_non_block_device` | S_IFBLK check on /dev/null |
| `valid_config_with_explicit_blocks` | Happy path construction |

### Integration Tests (require hardware, `#[ignore]`)

| Test | Validates |
|------|-----------|
| `initialize_opens_device` | Full init + shutdown lifecycle |
| `initialize_auto_detects_size` | BLKGETSIZE64 auto-detection |
| `initialize_errors_on_nonexistent_path` | Error on bad path |
| `initialize_rejects_non_block_device` | Error on /dev/null |
| `write_sync_read_sync_roundtrip` | Data integrity for sync IO |
| `async_read_write_completions` | Async IO with OpHandle tracking |
| `write_zeros_produces_zero_data` | WriteZeros correctness |
| `lba_out_of_range_error` | Bounds checking |
| `invalid_namespace_error` | ns_id validation |
| `ns_probe_returns_single_namespace` | NsProbe response |
| `unsupported_operations_return_not_supported` | NsCreate, ControllerReset |
| `device_info_methods` | All IBlockDevice query methods |
| `multiple_clients_independent_channels` | Multi-client isolation |
| `component_provides_iblock_device` | IUnknown interface query |
| `data_integrity_multi_block_patterns` | 64-block pattern write/verify |

### Benchmarks

| Benchmark | Measures |
|-----------|----------|
| `command_construction_latency` | Command enum creation overhead |
| `sync_io_latency/write_4k` | Sync write latency (4KB, real device) |
| `sync_io_latency/read_4k` | Sync read latency (4KB, real device) |
| `write_throughput/{1,8,32,128}` | Sequential write throughput at varying sizes |
| `read_throughput/{1,8,32,128}` | Sequential read throughput at varying sizes |

## Future Considerations

- **Multi-queue support**: Current design uses a single io_uring ring; could scale to per-CPU rings for higher IOPS.
- **Polling mode (IORING_SETUP_SQPOLL)**: Kernel-side SQ polling could reduce submission latency for high-frequency workloads.
- **io_uring registered buffers**: Pre-registering DMA buffers with `IORING_REGISTER_BUFFERS` could eliminate per-IO `get_user_pages` overhead.
- **NUMA affinity**: Currently returns -1; could detect device NUMA node and pin the actor thread accordingly.
- **Async write chaining**: Could link write + fsync SQEs via `IOSQE_IO_LINK` for atomic durable async writes (currently O_DSYNC handles durability implicitly).
- **Metrics per-client**: Telemetry currently tracks global stats; per-client breakdown would aid multi-tenant debugging.
- **io_uring probe**: Could probe kernel io_uring capabilities at init to adapt behavior to different kernel versions.

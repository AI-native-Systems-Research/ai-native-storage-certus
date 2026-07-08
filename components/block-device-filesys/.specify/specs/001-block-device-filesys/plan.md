# Implementation Plan: File-Backed Block Device (block-device-filesys)

**Branch**: `001-block-device-filesys` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The `block-device-filesys` component is a fully functional file-backed block device that implements `IBlockDevice` and `IBlockDeviceAdmin` from the Certus interfaces crate. It provides a drop-in replacement for the SPDK-backed NVMe driver, enabling development, testing, and benchmarking on any Linux machine without special hardware. The component uses the actor model with a dedicated thread running an io_uring event loop for async IO (with fallback to synchronous pread/pwrite), and multiplexes multiple client connections via lock-free SPSC channels.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-core` (workspace) — Actor model (`Actor`, `ActorHandle`, `ActorHandler`), SPSC channels (`SpscChannel`, `Sender`, `Receiver`), `IUnknown` trait
- `component-macros` (workspace) — `define_component!` proc macro
- `component-framework` (workspace) — Facade re-export
- `interfaces` (workspace, feature `spdk`) — `IBlockDevice`, `IBlockDeviceAdmin`, `Command`, `Completion`, `DmaBuffer`, `NvmeBlockError`, `OpHandle`, `TelemetrySnapshot`, `NamespaceInfo`, `ClientChannels`
- `io-uring` 0.7 — Linux io_uring async IO submission and completion ring
- `libc` 0.2 — `pread`, `pwrite`, `fdatasync`, `fallocate`, `posix_memalign`, `free`, `O_DIRECT`, `O_SYNC`
- `crossbeam-channel` 0.5 — Available but unused (primary channels come from `component-core::channel::spsc`)

**Dev Dependencies**:
- `criterion` 0.5 (with `html_reports`) — Benchmark framework
- `tempfile` 3 — Temporary directories for test isolation

## Architecture

### Component Layer

```
┌────────────────────────────────────────────────────────────────────┐
│  Upper-layer consumers (dispatcher, extent-manager, benchmarks)    │
│                                                                    │
│  ┌────────────┐   ┌────────────┐   ┌────────────┐                │
│  │  Client 0  │   │  Client 1  │   │  Client N  │                │
│  └─────┬──────┘   └─────┬──────┘   └─────┬──────┘                │
│        │                 │                 │                        │
│   SPSC Channels (capacity=64 per pair)                             │
│   cmd_tx/completion_rx   cmd_tx/completion_rx                      │
└────────┼─────────────────┼─────────────────┼───────────────────────┘
         │                 │                 │
┌────────┼─────────────────┼─────────────────┼───────────────────────┐
│  BlockDeviceFilesysComponent                                       │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Actor Thread (FilesysHandler)                               │   │
│  │                                                             │   │
│  │  ┌───────────────┐   ┌───────────────────┐                 │   │
│  │  │ Client Polling│──>│ Command Processing │                 │   │
│  │  │ (try_recv)    │   │ (process_command)  │                 │   │
│  │  └───────────────┘   └────────┬──────────┘                 │   │
│  │                               │                             │   │
│  │              ┌────────────────┼────────────────┐            │   │
│  │              v                v                v            │   │
│  │  ┌──────────────┐  ┌─────────────────┐  ┌──────────┐      │   │
│  │  │ Sync IO Path │  │ Async IO Path   │  │ Admin Ops│      │   │
│  │  │ pread/pwrite │  │ io_uring submit │  │ NsProbe  │      │   │
│  │  │ + fdatasync  │  │ + linked fsync  │  │ Abort    │      │   │
│  │  └──────┬───────┘  └────────┬────────┘  └──────────┘      │   │
│  │         │                   │                               │   │
│  │         v                   v                               │   │
│  │  ┌──────────────────────────────────┐                       │   │
│  │  │      Backing File (OwnedFd)      │                       │   │
│  │  │  O_DIRECT | O_SYNC (or buffered) │                       │   │
│  │  │  Pre-allocated via fallocate     │                       │   │
│  │  └──────────────────────────────────┘                       │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                    │
│  Optional: TelemetryStats (atomic counters, feature-gated)         │
└────────────────────────────────────────────────────────────────────┘
```

### Internal Module Structure

```
components/block-device-filesys/
├── Cargo.toml                    # Crate manifest with workspace deps
├── CLAUDE.md                     # AI assistant context pointer
├── src/
│   ├── lib.rs                    # Component definition (define_component!), IBlockDevice
│   │                             #   and IBlockDeviceAdmin impls, initialization, shutdown
│   ├── actor.rs                  # FilesysHandler (ActorHandler<ControlMessage>),
│   │                             #   ClientSession, ControlMessage, InflightOp,
│   │                             #   command processing, io_uring submit/harvest,
│   │                             #   timeout checking, sync IO fallback
│   ├── config.rs                 # DeviceConfig (validation), open_or_create_backing_file(),
│   │                             #   O_DIRECT fallback logic
│   └── telemetry.rs              # TelemetryStats (feature-gated), atomic op/latency/bytes
│                                 #   counters, snapshot generation
├── benches/
│   ├── latency.rs                # Criterion: command construction + sync read/write 4KB
│   └── throughput.rs             # Criterion: write zeros + read at 1/8/32/128 blocks
├── tests/
│   └── integration.rs            # Full integration test suite (12 tests)
└── .specify/
    └── specs/
        └── 001-block-device-filesys/
            ├── spec.md           # Feature specification (backfilled)
            ├── plan.md           # This file
            └── tasks.md          # Improvement task list
```

### Data Flow

**Initialization**:
1. Caller invokes `BlockDeviceFilesysComponent::create(path, block_size, num_blocks)` which stores config atomically.
2. Caller invokes `initialize()`:
   - Validates config via `DeviceConfig::new()` (power-of-2 block size >= 512, num_blocks > 0).
   - Opens/creates backing file via `open_or_create_backing_file()` — tries O_DIRECT|O_SYNC, falls back to buffered IO on EINVAL.
   - If file is new: pre-allocates via `fallocate`. If existing: verifies size matches.
   - Creates `FilesysHandler` which attempts to create an `IoUring` (depth=128), falling back to `None` if unavailable.
   - Wraps handler in `Actor::new()`, calls `activate()` to spawn the actor thread.
   - Stores the `ActorHandle<ControlMessage>` for client registration.

**Client Connection**:
1. Caller invokes `connect_client()` on the `IBlockDevice` trait.
2. Two `SpscChannel` pairs created (capacity=64 each): ingress (commands) and callback (completions).
3. A `ControlMessage::ConnectClient { session }` sent to actor via `ActorHandle::send()`.
4. Actor's `handle()` method inserts `ClientSession` into its `HashMap<u64, ClientSession>`.
5. Caller receives `ClientChannels { command_tx, completion_rx }`.

**IO Processing (actor idle loop)**:
1. `on_idle()` called by actor framework when no control messages pending.
2. `poll_clients()`: iterates all clients, `try_recv()` on each ingress channel (non-blocking).
3. For each received command, `process_command()` dispatches by variant:
   - **Sync**: pread/pwrite + fdatasync, immediate completion sent to client.
   - **Async (io_uring available)**: submit SQE (+ linked fsync for writes), track in `inflight` map.
   - **Async (fallback)**: perform sync pread/pwrite, send completion immediately.
   - **WriteZeros**: allocate 512-aligned zero buffer, pwrite + fdatasync.
   - **BatchSubmit**: process each sub-command sequentially.
   - **AbortOp**: issue `AsyncCancel` SQE, remove from inflight, send `AbortAck`.
   - **NsProbe**: return single namespace info.
   - **Unsupported** (NsCreate/NsDelete/NsFormat/ControllerReset): return `NotSupported` error.
4. `harvest_completions()`: drain io_uring CQ, match to inflight ops by user_data, send completions. Fsync CQEs (bit 63 set) are silently consumed.
5. `check_timeouts()`: scan inflight ops for expired deadlines, send `Timeout` completions, issue `AsyncCancel` SQEs.
6. Returns `true` (keep polling) if clients connected or ops inflight; `false` to park thread otherwise.

**Shutdown**:
1. `shutdown()` takes the `ActorHandle`, sends `ControlMessage::Shutdown`.
2. Actor sets `shutdown_requested = true`, `on_idle()` returns `false`.
3. `handle.deactivate()` joins the actor thread.

### Key Design Decisions

1. **Single actor thread multiplexing**: All IO is serialized on one thread. This simplifies correctness (no lock contention on the file descriptor) and matches the single-queue NVMe model for testing purposes.

2. **io_uring with linked fsync for durability**: Async writes submit a write SQE linked (IO_LINK) to a Fsync(DATASYNC) SQE. The fsync completion is identified by bit 63 in user_data and discarded silently. This simulates NVMe write completion semantics (data is durable on completion).

3. **Graceful io_uring fallback**: If `IoUring::new(128)` fails (old kernel, container restrictions), the ring is `None` and all async commands degrade to synchronous pread/pwrite. A warning is logged but no error returned.

4. **O_DIRECT with buffered fallback**: The backing file is opened with O_DIRECT|O_SYNC to bypass the kernel page cache. If the filesystem returns EINVAL (e.g., tmpfs), it falls back to buffered IO with a stderr warning, enabling tests on any tmpdir.

5. **Non-blocking client polling**: The actor uses `try_recv()` on all client ingress channels, avoiding blocking/stalling other clients while one is idle.

6. **DMA buffer alignment**: All buffers for O_DIRECT IO are 512-byte aligned via `posix_memalign`. WriteZeros allocates its own aligned buffer internally.

7. **Feature-gated telemetry**: When compiled without `--features telemetry`, the `TelemetryStats` struct and all recording code is entirely excluded at compile time (no runtime cost). When enabled, atomic counters track ops, bytes, and latency.

8. **Monotonically increasing OpHandles**: Each submitted operation gets a unique `OpHandle(u64)` via `next_handle.wrapping_add(1)`, starting at 1.

## Dependencies

| Crate | Version | Purpose | Build Gate |
|-------|---------|---------|------------|
| `component-core` | workspace | Actor, SPSC channels, IUnknown | always |
| `component-macros` | workspace | `define_component!` macro | always |
| `component-framework` | workspace | Facade re-export | always |
| `interfaces` | workspace + `spdk` feature | IBlockDevice, IBlockDeviceAdmin, Command, Completion, DmaBuffer, error types | always |
| `io-uring` | 0.7 | Async IO submission/completion | always (runtime fallback if unavailable) |
| `libc` | 0.2 | Syscalls: pread, pwrite, fdatasync, fallocate, posix_memalign, free | always |
| `crossbeam-channel` | 0.5 | Available (unused; channels come from component-core) | always |
| `criterion` | 0.5 | Benchmark harness | dev only |
| `tempfile` | 3 | Test/bench temp directories | dev only |

## Testing

### Unit Tests (in `src/lib.rs`)
- `component_version` — version string matches Cargo.toml
- `component_provides_iblock_device` — IUnknown advertises IBlockDevice
- `component_has_logger_receptacle` — logger receptacle is declared
- `connect_client_not_initialized` — connect_client before init returns NotInitialized
- `device_info_returns_configured_values` — block_size, max_queue_depth, num_io_queues, etc.
- `sector_size_invalid_namespace` — ns_id != 1 returns InvalidNamespace
- `num_sectors_valid` — num_sectors(1) returns configured count
- `telemetry_not_available_without_feature` — telemetry() returns FeatureNotEnabled

### Unit Tests (in `src/config.rs`)
- `valid_config` — DeviceConfig with valid params
- `block_size_too_small` — < 512 rejected
- `block_size_not_power_of_two` — non-power-of-2 rejected
- `zero_blocks` — num_blocks=0 rejected
- `default_block_size_512` — 512 minimum works

### Integration Tests (`tests/integration.rs`)
- `initialize_creates_backing_file` — file created with correct size
- `initialize_errors_on_size_mismatch` — existing file with wrong size rejected
- `write_sync_read_sync_roundtrip` — data integrity for sync path
- `async_read_write_completions` — async path produces correct completions with handles
- `write_zeros_produces_zero_data` — zeros overwrite non-zero data
- `lba_out_of_range_error` — LBA beyond device returns error
- `invalid_namespace_error` — ns_id != 1 returns error
- `ns_probe_returns_single_namespace` — probe returns ns_id=1 with correct geometry
- `unsupported_operations_return_not_supported` — NsCreate, ControllerReset rejected
- `device_info_methods` — all IBlockDevice info methods return expected values
- `multiple_clients_independent_channels` — two clients share backing store, independent completions
- `invalid_file_path_initialization_error` — non-existent parent dir errors
- `data_integrity_multi_block_patterns` — 64-block write/read/overwrite integrity
- `async_tag_propagation` — caller tags echoed in async completions
- `component_provides_iblock_device` — IUnknown integration check

### Benchmarks
- `latency.rs`: command construction overhead (WriteZeros at depth 1/4/16/64), sync write 4KB latency, sync read 4KB latency
- `throughput.rs`: write throughput (WriteZeros at 1/8/32/128 blocks), read throughput (ReadSync at 1/8/32/128 blocks)

### Running Tests
```bash
cargo test -p block-device-filesys           # Unit + integration tests
cargo test -p block-device-filesys --features telemetry  # With telemetry enabled
cargo bench -p block-device-filesys --bench latency
cargo bench -p block-device-filesys --bench throughput
```

## Future Considerations

1. **Vectored IO (readv/writev)**: Multi-block IO currently does a single large pread/pwrite. Scatter-gather would reduce copy overhead for multi-buffer operations.

2. **io_uring batching**: Currently each async command submits immediately. Batching multiple SQEs before a single `submit()` call could reduce syscall overhead under load.

3. **Actor park/wake optimization**: When no clients are connected, the actor returns `false` from `on_idle()` causing it to park on the framework's condvar. The transition in/out of parked state adds latency for the first command after idle. A spin-then-park strategy could reduce tail latency.

4. **Disconnect client support from outside**: Currently `DisconnectClient` is only sent internally. Exposing a `disconnect_client(id)` method on the component would allow upper layers to clean up sessions.

5. **True async timeout enforcement**: The timeout checking relies on `on_idle()` polling frequency. Under heavy io_uring load, timeout precision depends on how quickly CQE harvesting completes. A separate timer mechanism (io_uring timeout SQE) would be more precise.

6. **Multi-namespace support**: Currently hardcoded to ns_id=1. Supporting multiple virtual namespaces (partitions of the backing file) would improve testing fidelity for multi-namespace NVMe controllers.

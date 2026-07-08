# Implementation Plan: Block Device Kernel (io_uring)

**Branch**: `001-block-device-kernel` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The `block-device-kernel` component provides a Linux kernel-path block device driver implementing `IBlockDevice` and `IBlockDeviceAdmin` via io_uring. It enables Certus to access raw NVMe (or other Linux block) devices without requiring the SPDK userspace driver, using `O_DIRECT | O_DSYNC` for cache bypass and write durability. The component follows the Certus actor model with a dedicated thread owning the io_uring instance and serving multiple clients through lock-free SPSC channels.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-core` (workspace) -- Actor model, `ActorHandler` trait, SPSC channels, `IUnknown`
- `component-macros` (workspace) -- `define_component!` proc macro
- `component-framework` (workspace) -- Facade re-export
- `interfaces` (workspace, features = ["spdk"]) -- `IBlockDevice`, `IBlockDeviceAdmin`, `Command`, `Completion`, `DmaBuffer`, `NvmeBlockError`, `TelemetrySnapshot`
- `io-uring` 0.7 -- io_uring syscall wrapper (submission/completion queues, opcodes)
- `libc` 0.2 -- `posix_memalign`, `posix_fadvise`, `ioctl`, `stat`, `fcntl`, `O_DIRECT`, `O_DSYNC`
- `crossbeam-channel` 0.5 -- Actor MPSC control channel (transitional)

**Dev Dependencies**:
- `criterion` 0.5 (html_reports) -- Benchmark framework

**Platform**: Linux only (io_uring >= 5.1, `O_DIRECT`, `BLKGETSIZE64` ioctl, `posix_memalign`)

## Architecture

### Component Layer

```
 ┌──────────────────────────────────────────────────────────────────────┐
 │                     BlockDeviceKernelComponent                        │
 │  (define_component! macro: IBlockDevice + IBlockDeviceAdmin)         │
 │                                                                      │
 │  ┌─────────────┐   ┌──────────────┐   ┌────────────────────────┐   │
 │  │ DeviceConfig │   │ AtomicU32/64 │   │ Mutex<ActorHandle>     │   │
 │  │ (path, bs,  │   │ (block_size, │   │ (control channel to    │   │
 │  │  num_blocks) │   │  num_blocks, │   │  KernelHandler actor)  │   │
 │  └─────────────┘   │  client_id)  │   └────────────────────────┘   │
 │                     └──────────────┘                                 │
 └─────────────────────────────┬────────────────────────────────────────┘
                               │ ControlMessage (ConnectClient/Shutdown)
                               ▼
 ┌──────────────────────────────────────────────────────────────────────┐
 │                         KernelHandler (Actor)                         │
 │  ActorHandler<ControlMessage> on dedicated OS thread                  │
 │                                                                      │
 │  ┌─────────┐  ┌──────────────┐  ┌─────────────────────────────────┐│
 │  │ OwnedFd │  │ IoUring(128) │  │ HashMap<u64, ClientSession>     ││
 │  │ (O_DIRECT  │ (SQ depth    │  │  - id: u64                      ││
 │  │  O_DSYNC)  │  = 128)      │  │  - ingress_rx: Receiver<Cmd>    ││
 │  └─────────┘  └──────────────┘  │  - callback_tx: Sender<Compl>   ││
 │                                  └─────────────────────────────────┘│
 │  ┌─────────────────────────────────────────────────────────────────┐│
 │  │ HashMap<u64, InflightOp>  (async ops keyed by op handle)        ││
 │  │  - handle, client_id, deadline, is_read, [telemetry fields]     ││
 │  └─────────────────────────────────────────────────────────────────┘│
 └──────────────────────────────────────────────────────────────────────┘
                               │
                               │ io_uring SQE/CQE
                               ▼
 ┌──────────────────────────────────────────────────────────────────────┐
 │                    Linux Kernel Block Layer                           │
 │           /dev/nvme0n1  |  /dev/sda  |  etc.                         │
 └──────────────────────────────────────────────────────────────────────┘
```

### Internal Module Structure

```
components/block-device-kernel/
├── Cargo.toml                       # Crate manifest with [features] telemetry
├── CLAUDE.md                        # Agent context pointer
├── src/
│   ├── lib.rs                       # Component definition (define_component!),
│   │                                #   IBlockDevice + IBlockDeviceAdmin impls,
│   │                                #   initialize()/shutdown() lifecycle,
│   │                                #   connect_client() channel wiring
│   ├── actor.rs                     # KernelHandler (ActorHandler<ControlMessage>),
│   │                                #   ClientSession, ControlMessage enum,
│   │                                #   InflightOp, command processing,
│   │                                #   io_uring submission/completion,
│   │                                #   timeout checking, abort handling
│   ├── config.rs                    # DeviceConfig validation, open_block_device()
│   │                                #   with O_DIRECT|O_DSYNC, BLKGETSIZE64 ioctl,
│   │                                #   stat-based block device assertion
│   └── telemetry.rs                 # TelemetryStats (feature-gated): atomic
│                                    #   counters for ops/latency/bytes, snapshot()
├── tests/
│   └── integration.rs              # Hardware-dependent integration tests (#[ignore])
│                                    #   roundtrip, async, write-zeros, errors,
│                                    #   multi-client, data integrity
└── benches/
    ├── latency.rs                   # Criterion: sync read/write 4KB latency
    └── throughput.rs                # Criterion: sequential read/write at 1/8/32/128 blocks
```

### Data Flow

**Initialization**:
1. `BlockDeviceKernelComponent::create(path, block_size, num_blocks)` stores config atomics
2. `initialize()` validates via `DeviceConfig::new()` (block_size >= 512, power-of-2, auto-detect via BLKGETSIZE64 if num_blocks=0)
3. `open_block_device()` opens with `O_DIRECT | O_DSYNC`, verifies via `fcntl(F_GETFL)`, invalidates page cache via `posix_fadvise(POSIX_FADV_DONTNEED)`
4. Creates `IoUring::new(128)` ring
5. Constructs `KernelHandler`, wraps in `Actor`, calls `activate()` to spawn dedicated thread
6. Stores `ActorHandle` for control channel access

**Client Connection**:
1. `connect_client()` allocates two SPSC channels (capacity 64 each): ingress (commands) and callback (completions)
2. Sends `ControlMessage::ConnectClient { session }` to actor via control channel
3. Returns `ClientChannels { command_tx, completion_rx }` to caller

**Synchronous IO (ReadSync/WriteSync)**:
1. Actor `poll_clients()` drains ingress channels in `on_idle()`
2. `process_command()` dispatches to `handle_read_sync()` or `handle_write_sync()`
3. Validates LBA range (ns_id=1, lba+num_blocks <= device_num_blocks)
4. Builds io_uring SQE (Read/Write opcode) with user_data = op handle
5. Calls `ring.submit_and_wait(1)` -- blocks until CQE arrives
6. `wait_for_cqe(handle)` loops over CQ, processes unrelated async CQEs, returns when target CQE found
7. Sends `Completion::ReadDone` / `Completion::WriteDone` on callback channel

**Asynchronous IO (ReadAsync/WriteAsync)**:
1. Same validation and SQE construction as sync path
2. Calls `ring.submit()` (non-blocking)
3. Inserts `InflightOp` into inflight map with deadline = now + timeout_ms
4. Completion delivered later via `harvest_completions()` in `on_idle()`

**Idle Loop** (`on_idle()` returns true while clients exist or ops are inflight):
1. `poll_clients()` -- drain all client ingress channels
2. `harvest_completions()` -- process all pending CQEs from io_uring CQ
3. `check_timeouts()` -- expire operations past deadline, send `Timeout` completion, issue `AsyncCancel`

**Shutdown**:
1. `shutdown()` sends `ControlMessage::Shutdown` to actor
2. `handle()` sets `shutdown_requested = true`
3. `on_idle()` returns `false`, actor thread exits
4. `handle.deactivate()` joins actor thread

### Key Design Decisions

1. **io_uring only**: No pread/pwrite fallback. Simplifies the code path and ensures all IO benefits from io_uring's batching and kernel-bypass submission.

2. **O_DIRECT | O_DSYNC**: Bypasses page cache for predictable latency (no cache pollution) and guarantees write durability on completion without separate fsync calls.

3. **Single actor thread**: One OS thread per component instance owns the io_uring ring and fd. Avoids concurrency complexity within the IO path. Multiple clients share the same ring via SPSC channels.

4. **SPSC channels (capacity 64)**: Bounded channels prevent unbounded memory growth while allowing pipelining. One channel pair per client ensures isolation -- no cross-contamination of completions.

5. **posix_memalign for WriteZeros**: Uses heap-allocated 512-byte-aligned zero buffer rather than `fallocate(FALLOC_FL_ZERO_RANGE)` because O_DIRECT fds may not support fallocate on all block device types.

6. **Feature-gated telemetry**: Zero-cost when disabled (compile-time `#[cfg(feature = "telemetry")]` gating). Uses lock-free atomics with CAS loops for min/max tracking.

7. **wait_for_cqe() processes unrelated CQEs**: While waiting for a synchronous operation's specific CQE, the handler also processes any async CQEs that arrive, avoiding starvation of async completions during sync operations.

8. **BLKGETSIZE64 ioctl (x86_64)**: Uses hardcoded ioctl constant `0x80081272`. Not portable to other architectures without conditional compilation.

9. **BatchSubmit sequential**: Processed via recursive `process_command()` calls within the actor -- does not leverage io_uring multi-SQE atomic batching.

10. **Namespace = 1 only**: Kernel block devices map to a single logical namespace (ns_id=1). All other ns_ids return `InvalidNamespace`.

## Dependencies

| Dependency | Version | Purpose | Workspace |
|-----------|---------|---------|-----------|
| `component-core` | workspace | Actor model, ActorHandler trait, SPSC channels, IUnknown | Yes |
| `component-macros` | workspace | `define_component!` proc macro | Yes |
| `component-framework` | workspace | Facade re-export | Yes |
| `interfaces` | workspace (features=["spdk"]) | IBlockDevice, IBlockDeviceAdmin, Command, Completion, DmaBuffer, NvmeBlockError, TelemetrySnapshot, OpHandle, NamespaceInfo, ClientChannels | Yes |
| `io-uring` | 0.7 | io_uring syscall wrapper (submission/completion queues, opcodes) | No |
| `libc` | 0.2 | posix_memalign, posix_fadvise, ioctl, stat, fcntl, O_DIRECT, O_DSYNC, close, free | No |
| `crossbeam-channel` | 0.5 | Actor MPSC control channel (via component-core) | No |
| `criterion` | 0.5 (dev) | Benchmark framework with HTML reports | No |

**Runtime Requirements**:
- Linux kernel >= 5.1 (io_uring support)
- Raw block device accessible by the running user (e.g., `/dev/nvme0n1`)
- Sufficient `memlock` ulimit for io_uring memory mapping

## Testing

### Unit Tests (`src/lib.rs`, `src/config.rs`)
- Component version, provided interfaces, receptacles (no hardware needed)
- `connect_client()` before `initialize()` returns `NotInitialized`
- Device info methods return configured values
- Invalid namespace errors
- Config validation: block_size < 512, non-power-of-two, non-block-device path

### Integration Tests (`tests/integration.rs`, `#[ignore]` -- requires real device)
- `initialize_opens_device` -- basic open and shutdown
- `initialize_auto_detects_size` -- BLKGETSIZE64 auto-detection
- `initialize_errors_on_nonexistent_path` -- error path (no hardware)
- `initialize_rejects_non_block_device` -- /dev/null rejection (no hardware)
- `write_sync_read_sync_roundtrip` -- data integrity single block
- `async_read_write_completions` -- async path with timeout
- `write_zeros_produces_zero_data` -- zero fill + verify
- `lba_out_of_range_error` -- bounds checking
- `invalid_namespace_error` -- ns_id != 1 rejection
- `ns_probe_returns_single_namespace` -- probe response
- `unsupported_operations_return_not_supported` -- NsCreate, ControllerReset
- `device_info_methods` -- all IBlockDevice introspection methods
- `multiple_clients_independent_channels` -- multi-client isolation + cross-read
- `data_integrity_multi_block_patterns` -- 64-block unique-pattern roundtrip

### Benchmarks (`benches/`)
- `latency.rs`: Command construction latency; sync read/write 4KB latency (real device)
- `throughput.rs`: Sequential write throughput (WriteZeros) and read throughput at 1/8/32/128 blocks (real device)

### Running Tests
```bash
# Unit tests (no hardware required)
cargo test -p block-device-kernel

# Integration tests (requires block device, default /dev/nvme0n1)
cargo test -p block-device-kernel --test integration -- --ignored

# Override device path
TEST_BLOCK_DEVICE=/dev/sdb cargo test -p block-device-kernel --test integration -- --ignored

# Benchmarks (requires block device)
BENCH_DEVICE_PATH=/dev/nvme0n1 cargo bench -p block-device-kernel
```

## Future Considerations

1. **io_uring batching**: `BatchSubmit` currently processes commands sequentially. A true multi-SQE batch submission could improve throughput for batch workloads.
2. **Architecture portability**: The `BLKGETSIZE64` ioctl constant is x86_64-specific. Conditional compilation or a `nix` crate wrapper would enable ARM64/other architectures.
3. **CPU pinning**: The `set_actor_cpu()` admin method is currently a no-op. Implementing thread affinity via `sched_setaffinity` would enable NUMA-aware placement.
4. **Latency telemetry accuracy**: The telemetry module currently passes `0` for latency_ns in most `record_op()` calls within `actor.rs`. Proper per-operation timing (using the `start_ns` field in `InflightOp`) should be propagated.
5. **Graceful client disconnect**: No explicit disconnect-on-drop mechanism exists. If a client drops its channel endpoints, the actor only detects this on the next failed `send()`.
6. **io_uring polling mode**: For lowest latency, `IORING_SETUP_SQPOLL` could be explored to avoid syscall overhead on submission.
7. **Multiple namespaces**: If Linux multi-queue block devices need distinct namespace semantics, the single ns_id=1 assumption would need extension.

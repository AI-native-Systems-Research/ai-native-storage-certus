# Implementation Plan: SPDK NVMe Block Device

**Branch**: `001-block-device-spdk-nvme` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The `block-device-spdk-nvme` component is a high-performance NVMe block device driver that bypasses the kernel storage stack entirely via SPDK userspace NVMe access. It follows the Certus actor-based component model: each component instance owns a single NVMe controller (identified by PCI address), runs a dedicated actor thread NUMA-pinned to the controller's memory domain, and communicates with clients through lock-free SPSC channel pairs. The component supports synchronous and asynchronous IO, batch submission, namespace management, controller reset, operation abort, and optional compile-time telemetry.

## Technical Context

- **Platform**: Linux x86_64 only (requires invariant TSC, VFIO/IOMMU, hugepages, memlock unlimited)
- **SPDK**: Userspace NVMe driver via raw FFI bindings (`spdk-sys` crate, bindgen-generated)
- **Component Framework**: `define_component!` macro generates IUnknown, receptacle wiring, and interface queries
- **Actor Model**: Single dedicated OS thread per controller, spinning (never sleeping) to poll completions
- **Zero-copy IO**: DMA buffers allocated from SPDK hugepages; `Arc<DmaBuffer>` passed in messages
- **Channel Transport**: Per-client SPSC channels (256-slot capacity) for command/completion flow
- **Feature Flags**: `telemetry` (opt-in latency/throughput statistics), `spdk` (gate for integration tests and benchmarks)

## Architecture

### Component Layer

```
+------------------------------------------------------------------+
|                         Client Threads                            |
|  +----------+  +----------+  +----------+                        |
|  | Client 0 |  | Client 1 |  | Client N |  (via connect_client) |
|  +----+-----+  +----+-----+  +----+-----+                       |
|       |              |              |                             |
|  command_tx     command_tx     command_tx   (SPSC, 256 slots)    |
|       |              |              |                             |
+-------+--------------+--------------+----------------------------+
        |              |              |
        v              v              v
+------------------------------------------------------------------+
|              BlockDeviceHandler (Actor Thread)                    |
|  +-----------------------------------------------------------+   |
|  | on_idle() loop:                                           |   |
|  |   1. poll_clients() [round-robin, rotating start_idx]     |   |
|  |   2. process_completions() [all qpairs]                   |   |
|  |   3. drain async_completions -> callback channels         |   |
|  |   4. check_timeouts() [~1ms throttled via TSC]            |   |
|  +-----------------------------------------------------------+   |
|       |              |              |                             |
|  completion_rx  completion_rx  completion_rx (SPSC, 256 slots)   |
|       |              |              |                             |
+-------+--------------+--------------+----------------------------+
        v              v              v
+------------------------------------------------------------------+
|                         Client Threads                            |
+------------------------------------------------------------------+

+------------------------------------------------------------------+
|              NVMe Controller (via SPDK FFI)                       |
|  +-------------------+  +-------------------+                    |
|  |  QueuePair (QD=4) |  | QueuePair (QD=16) |                    |
|  +-------------------+  +-------------------+                    |
|  +-------------------+  +--------------------+                   |
|  | QueuePair (QD=64) |  | QueuePair (QD=256) |                   |
|  +-------------------+  +--------------------+                   |
|  +-----------------------------------------------------------+   |
|  |  Namespace 1  |  Namespace 2  |  ...  |  Namespace N      |   |
|  +-----------------------------------------------------------+   |
+------------------------------------------------------------------+
```

### Internal Module Structure

```
components/block-device-spdk-nvme/
  Cargo.toml                   # Crate manifest (features: telemetry, spdk)
  src/
    lib.rs                     # Component definition (define_component!), IBlockDevice
                               # + IBlockDeviceAdmin impls, probe_controller(), connect_client()
    actor.rs                   # BlockDeviceHandler (ActorHandler<ControlMessage>):
                               #   poll_clients(), dispatch_command(), check_timeouts(),
                               #   handle_controller_reset(), on_idle/on_start/on_stop,
                               #   sync/async IO submission, ENOMEM backpressure retry,
                               #   ContextPool (slab allocator for AsyncIoContext),
                               #   async_completion_cb / sync_completion_cb (extern "C")
    controller.rs              # NvmeController: attach/detach lifecycle, namespace discovery,
                               #   device info queries, NvmeVersion, NvmeNamespaceInfo
    qpair.rs                   # QueuePair + QueuePairPool: allocate from SPDK, depth tracking,
                               #   shallowest-fit selection heuristic, process_completions()
    namespace.rs               # Namespace ops: create/format/delete via SPDK admin commands,
                               #   validate_ns_id(), validate_lba_range(), capacity queries
    tsc.rs                     # TscClock: rdtsc calibration (2ms window), ticks_to_ns(),
                               #   deadline_from_ms(), has_elapsed()
    telemetry.rs               # TelemetryStats (feature-gated): atomic counters, CAS min/max,
                               #   TelemetrySnapshot generation
    command.rs                 # Internal types: ClientSession, ControlMessage enum
  tests/
    integration.rs             # Hardware integration tests (OnceLock<SpdkHardwareContext>),
                               #   auto-skip when SPDK unavailable, atexit(_exit(0)) safety
  benches/
    latency.rs                 # Criterion: sync IO latency at QD 1/4/16/64
    throughput.rs              # Criterion: async IO throughput at varying batch sizes
```

### Data Flow / Key Paths

**Synchronous Read/Write Path**:
1. Client sends `Command::ReadSync { ns_id, lba, buf }` on `command_tx`
2. Actor receives in `poll_clients()`, calls `do_sync_read()`
3. `do_sync_read()`: validates ns_id + LBA range, gets ns_ptr via SPDK
4. Allocates `SyncCompletionCtx` on stack, submits `spdk_nvme_ns_cmd_read()`
5. Spins calling `qp.process_completions(0)` until `ctx.done` is set by `sync_completion_cb`
6. Checks NVMe status (sct/sc), sends `Completion::ReadDone` on `callback_tx`

**Asynchronous Read/Write Path**:
1. Client sends `Command::ReadAsync { ns_id, lba, buf, timeout_ms, tag }`
2. Actor: validates ns/LBA, selects qpair via `select_index(pending_ops.len() + 1)`
3. Acquires `AsyncIoContext` from `ContextPool`, fills fields (client_id, handle, start TSC)
4. Submits `spdk_nvme_ns_cmd_read()` with `async_completion_cb` and raw context pointer
5. On `-ENOMEM`: retry loop polling completions up to `min(timeout_ms, 1000ms)`
6. Inserts `PendingOp` with TSC-based deadline
7. Later: `process_completions()` fires `async_completion_cb` which pushes `AsyncCompletionEntry`
8. Actor drains entries, matches to client, sends `Completion::ReadDone { handle, tag, result }`

**Timeout Path**:
1. `check_timeouts()` called ~every 1ms (TSC-throttled in `on_idle()`)
2. Iterates all clients' `pending_ops`, compares deadline against `tsc.now()`
3. Expired ops: removed from `pending_ops`, `Completion::Timeout { handle }` sent

**Controller Reset Path**:
1. Client sends `Command::ControllerReset`
2. Intercepted in `poll_clients()` before `dispatch_command()`
3. Cancels ALL pending ops across ALL clients with `Completion::Error { Aborted }`
4. Calls `spdk_nvme_ctrlr_reset()`, refreshes namespaces
5. Sends `Completion::ResetDone` to requesting client

**Shutdown Path**:
1. `signal_stop()` closes actor command channel, `on_idle()` returns false
2. `on_stop()`: drains all qpairs (5s timeout), sends `Error(Aborted)` to all pending
3. Parks controller in `Arc<Mutex<Option<NvmeController>>>` (outlives actor thread)
4. `shutdown()`: calls `handle.deactivate()` to join actor thread
5. `detach_controller()`: takes controller from parking slot, Drop calls `spdk_nvme_detach`

### Key Design Decisions

1. **Shallowest-fit QueuePair Selection**: The pool selects the shallowest queue with sufficient capacity for the batch size. This minimizes hardware latency for small IO (shallow queues have fewer pending commands to drain before reaching ours) while providing throughput capacity for large batches. Fallback: most-available-capacity queue (not unconditionally deepest) to spread load.

2. **Controller Parking on Shutdown**: The controller is moved to a shared `Arc<Mutex<Option<NvmeController>>>` during `on_stop()` rather than dropped. This ensures `spdk_nvme_detach` is called AFTER the actor thread exits, which is required because SPDK may still be processing completions when the thread joins.

3. **ENOMEM Backpressure Retry**: On async submit returning `-ENOMEM` (qpair request pool exhausted), the actor retries by polling completions to free slots, bounded by `min(timeout_ms, 1000ms)`. This converts transient saturation under concurrent load into brief backpressure rather than spurious failures.

4. **TSC-based Timing**: The actor uses `rdtsc` (calibrated once at construction via 2ms spin against `clock_gettime`) for all hot-path timing. This avoids the 50-80 cycle overhead of `clock_gettime` vDSO calls on every deadline check and telemetry measurement. Cost: ~20 cycles.

5. **ContextPool Slab Allocation**: Pre-allocated pool of 340 `AsyncIoContext` objects eliminates per-IO heap allocation on the async hot path. Contexts are leaked via `Box::into_raw` for SPDK callbacks and reclaimed via `Box::from_raw` in the completion callback.

6. **Spinning Actor (Never Parks)**: Whenever clients are connected, `on_idle()` returns `true` to keep the actor spinning. NVMe completions only arrive via `spdk_nvme_qpair_process_completions()` which must be called actively; parking would delay completion delivery and collapse effective queue depth.

7. **Round-Robin Client Polling**: A rotating `poll_start_idx` ensures fair access across clients, preventing head-of-line blocking where one busy client starves others.

8. **`io_queue_requests = depth * 4`**: Queue pair allocation sets the SPDK internal request tracker pool to 4x the queue depth. This absorbs request splitting (large IO split into multiple NVMe commands by SPDK) and transient bursts without premature `-ENOMEM`.

9. **Feature-Gated Telemetry**: The `telemetry` feature adds zero overhead when disabled (conditional compilation, not runtime check). When enabled, uses only atomic `fetch_add` and `compare_exchange_weak` with `Relaxed` ordering — no locks on the hot path.

10. **OnceLock + atexit in Tests**: SPDK is a process singleton. Integration tests use `OnceLock<Option<&'static SpdkHardwareContext>>` for one-time init and `atexit(|| _exit(0))` to prevent SIGSEGV from SPDK/DPDK's own teardown after Arc-leaked components.

## Dependencies

| Dependency | Role | Integration Point |
|---|---|---|
| `component-framework` | Framework facade | `define_component!`, Actor, SpscChannel, NUMA topology |
| `component-core` | Core traits | `IUnknown`, `ActorHandler`, `ActorHandle`, `Receiver`/`Sender`, `bind()` |
| `component-macros` | Proc macros | Interface/component code generation |
| `interfaces` (features=["spdk"]) | Shared types | `IBlockDevice`, `IBlockDeviceAdmin`, `Command`, `Completion`, `DmaBuffer`, `PciAddress`, `NvmeBlockError`, `ClientChannels`, `OpHandle`, `TelemetrySnapshot`, `NamespaceInfo` |
| `spdk-sys` | FFI bindings | All `spdk_nvme_*` function calls (probe, attach, read, write, qpair, admin cmds) |
| `spdk-env` | Safe SPDK init | `ISPDKEnv` receptacle — SPDK environment must be initialized before probe |
| `logger` | Logging | `ILogger` receptacle — debug/info logging from actor and component |
| `crossbeam-channel` | Channels | Used internally (bounded channels for component-core integration) |
| `criterion` (dev) | Benchmarks | Latency/throughput benchmarks at varying queue depths |

## Testing

| Level | What | Hardware Required |
|---|---|---|
| Unit tests (`cargo test`) | Component metadata, interface queries, receptacle presence, error paths, defaults, telemetry snapshot types, QueuePair/Pool selection, TscClock calibration, namespace validation, NvmeVersion display | No |
| Integration tests (`tests/integration.rs`) | Full wiring + binding, SPDK init, controller probe/attach, sync/async read/write, namespace CRUD, controller reset, multi-client concurrency, timeout/abort | Yes (auto-skip) |
| Doc tests (`cargo test --doc`) | `NvmeVersion` Display, `NvmeNamespaceInfo::capacity_bytes()`, `QueuePairPool::select_index()` | No |
| Benchmarks (`cargo bench`) | Sync IO latency at QD 1/4/16/64, async IO throughput at varying batch sizes, command construction overhead | Yes (skip group if unavailable) |

**Test strategy for hardware-dependent code**: Integration tests use `OnceLock<Option<&'static SpdkHardwareContext>>` for runtime detection. If SPDK hardware is unavailable (no VFIO, no hugepages, no NVMe), tests pass with an explanatory `eprintln!` and early return. CI runs unit tests only (no SPDK hardware); hardware tests run locally or in dedicated CI runners.

## Future Considerations

1. **Multi-Namespace IO Isolation**: Current implementation shares the QueuePairPool across all namespaces. Per-namespace queue pair affinity could reduce cross-namespace contention for multi-tenant workloads.

2. **Completion Batching**: Individual completions are sent one-at-a-time on callback channels. A batch-completion API could reduce channel pressure when many async ops complete in a single `process_completions()` call.

3. **Hot-Path Allocation Elimination**: The `HashMap<u64, PendingOp>` per client still allocates on insert/remove. A slab allocator (like ContextPool) for PendingOp could eliminate this.

4. **Adaptive Queue Pair Selection**: The current shallowest-fit heuristic is static. An adaptive approach that considers recent completion rates or latency percentiles could improve selection under mixed workloads.

5. **NVMe-oF Transport Support**: The current implementation is PCIe-only (`SPDK_NVME_TRANSPORT_PCIE`). Adding NVMe-oF (TCP, RDMA) transport support would enable remote NVMe access using the same component interface.

6. **Write Buffer Coalescing**: Small writes could be coalesced into larger NVMe commands to improve throughput for workloads with many small sequential writes.

7. **Priority Classes**: Differentiated queue pair assignment by IO priority (latency-sensitive reads on shallow queues, background writes on deep queues) regardless of batch size.

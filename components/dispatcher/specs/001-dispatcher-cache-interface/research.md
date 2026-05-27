# Research: Dispatcher Cache Interface

**Date**: 2026-04-28 | **Plan**: [plan.md](plan.md)

## R-1: Component Framework Receptacle Limitations for N Dependencies

**Decision**: Use `IDispatcher::initialize()` parameters for N data block devices and N extent managers; keep only logger and dispatch_map as receptacles.

**Rationale**: The `define_component!` macro generates one `Receptacle<T>` field per declaration — each is a single slot. For a runtime-determined N, there is no way to declare N receptacles at compile time. The functional design explicitly states PCI addresses are "passed as a parameter to the initialization function," confirming the initialize-time wiring pattern. The extent-manager and dispatch-map components both use single-slot receptacles, so this pattern is consistent.

**Alternatives considered**:
- Vec-based receptacle extension to the framework — rejected (framework modification out of scope)
- Fixed-count receptacles (e.g., 4 data devices) — rejected (limits scalability, violates design intent)

## R-2: MDTS Segmentation Pattern

**Decision**: Dispatcher queries `IBlockDevice::max_transfer_size()` per device at init time, stores the value, and splits I/O before submission using `Command::BatchSubmit`.

**Rationale**: Research confirmed the block-device-spdk-nvme component does NOT auto-split I/O. The `max_transfer_size` field is populated at controller attach time (default 131072 = 128 KiB). The `BatchSubmit { ops: Vec<Command> }` variant is designed exactly for submitting multiple sub-commands as a unit.

**Alternatives considered**:
- Auto-splitting in the block device actor — rejected (not implemented, would require framework changes)
- Single-command sequential submission — rejected (higher overhead than batching)

## R-3: Background Write Worker Pattern

**Decision**: Single background thread per dispatcher instance, using `std::thread::spawn` with `Arc<AtomicBool>` shutdown flag and channel-based job queue.

**Rationale**: The extent-manager v2 uses exactly this pattern for background checkpointing (`start_background_checkpoint`). The background thread receives write jobs via a channel, submits segmented I/O to the block device client, and on completion calls `dispatch_map.convert_to_storage()` to transition entries. On failure, it calls `dispatch_map.remove()` to clean up.

**Alternatives considered**:
- Actor pattern (dedicated SPDK thread) — rejected (dispatcher doesn't need direct SPDK access; it uses block device clients)
- Async runtime (tokio) — rejected (not used anywhere in the workspace; adds dependency complexity)
- Inline synchronous writes — rejected (violates spec FR-004: writes must be asynchronous)

## R-4: Dispatch Map State Machine Integration

**Decision**: Follow the dispatch-map's existing locking protocol: `take_write` during populate staging, `downgrade_reference` after staging completes, background thread uses `take_read` for SSD transfer then `take_write` for `convert_to_storage`.

**Rationale**: The `IDispatchMap` interface provides `take_read`, `take_write`, `release_read`, `release_write`, and `downgrade_reference` methods with blocking semantics (timeout built into the dispatch map). The lookup method blocks if a writer is active, which correctly serializes lookup against populate. Multiple concurrent lookups (reads) are allowed.

**Locking sequence for populate**:
1. `dispatch_map.take_write(key)` — exclusive access
2. `dispatch_map.create_staging(key, size)` — allocate staging buffer
3. DMA copy GPU → staging buffer
4. `dispatch_map.downgrade_reference(key)` — allow reads
5. Enqueue background write job
6. Background: `dispatch_map.take_write(key)` → write to SSD → `convert_to_storage(key, offset)` → `release_write(key)`

**Locking sequence for lookup**:
1. `dispatch_map.take_read(key)` — shared access (blocks if writer active)
2. `dispatch_map.lookup(key)` — get Staging or BlockDevice location
3. DMA copy staging/SSD → GPU
4. `dispatch_map.release_read(key)`

**Alternatives considered**:
- Custom Mutex + Condvar in dispatcher — rejected (duplicates dispatch map functionality, deadlock risk)

## R-5: IPC Handle and DMA Transfer Mechanism

**Decision**: The IPC handle is an opaque GPU memory descriptor (likely a CUDA IPC handle or similar). For the initial implementation, represent as a byte slice / raw pointer with size. Actual GPU DMA transfer implementation depends on the CUDA/GPU driver integration which is outside this component's scope.

**Rationale**: The functional design references "GPU memory (address/ipc-handle provided by the client)" without specifying the GPU framework. The dispatcher's role is to orchestrate the transfer flow, not implement the DMA engine. A type alias or struct in the interfaces crate keeps the API clean while deferring GPU-specific details.

**Alternatives considered**:
- Strongly-typed CUDA IPC handle — rejected (couples to specific GPU runtime)
- Passing raw pointers — rejected (unsafe API surface; prefer a typed wrapper)

## R-6: Block Device Client Usage Pattern

**Decision**: The dispatcher creates block device component instances during initialize, calls `IBlockDeviceAdmin::initialize()` on each, then obtains `ClientChannels` via `IBlockDevice::connect_client()` for I/O operations. Each background write uses the client channels (command_tx/completion_rx) with proper MDTS-segmented `BatchSubmit` commands.

**Rationale**: Confirmed by block-device-spdk-nvme v2 research. The `connect_client()` method returns channel endpoints for lock-free I/O submission. The actor handles command dispatch and completion notification.

**Alternatives considered**:
- Synchronous I/O via `ReadSync`/`WriteSync` — acceptable for SSD reads during lookup; background writes should use async for throughput

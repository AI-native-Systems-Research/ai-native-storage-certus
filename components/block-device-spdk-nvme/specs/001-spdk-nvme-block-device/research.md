# Research: SPDK NVMe Block Device Component

## R-001: Component-Framework Actor Pattern

**Decision**: Use the existing `Actor<M, H>` pattern from `component-core::actor` for the service thread, with a custom message enum as `M` and a handler struct as `H`.

**Rationale**: The component-framework already provides NUMA-aware actor pinning (`with_cpu_affinity`), lifecycle management (`on_start`/`on_stop`), and IUnknown integration. Building on this avoids duplicating thread management logic.

**Alternatives considered**:
- Raw `std::thread::spawn` with manual channel polling — rejected because the framework already handles affinity, panic recovery, and shutdown.
- Tokio async runtime — rejected because SPDK FFI is inherently synchronous and blocking; SPDK's threading model assumes dedicated polled threads.

## R-002: Channel Architecture for Client Communication

**Decision**: Use SPSC channels (from `component-core::channel::spsc::SpscChannel`) for each client's ingress and callback paths. Each client gets a `SpscChannel<Command>` (ingress) and a `SpscChannel<Completion>` (callback).

**Rationale**: The design specifies per-client shared-memory channels. SPSC is optimal for single-client-to-actor communication. The framework's `SpscChannel` is a lock-free ring buffer with IUnknown integration.

**Alternatives considered**:
- MPSC for ingress — rejected per design requirement of per-client channels.
- Crossbeam bounded channel — viable for benchmarking (design mentions "crossbeam bounded to 64 slots"); will use for test/bench configurations.

## R-003: Actor Thread Polling Multiple Channels

**Decision**: The actor handler will maintain a `Vec` of client channel receivers. On each `handle()` invocation the actor will poll all client ingress channels with `try_recv()`. Additionally, the main actor MPSC channel will be used for control messages (connect client, disconnect, shutdown).

**Rationale**: The framework's `Actor` uses a single MPSC inbound channel. Control messages (e.g., "new client connected") arrive via this channel. The handler then polls all client SPSC ingress channels in a tight loop. This two-tier approach lets the actor receive new client registrations dynamically while polling existing clients without blocking.

**Alternatives considered**:
- Single MPSC for all clients — rejected because it prevents per-client backpressure and doesn't match the design's "two channels per client" requirement.
- `select!`-style polling — no suitable non-async select primitive; manual `try_recv` polling is standard for SPDK-style polled architectures.

## R-004: SPDK NVMe API Integration

**Decision**: Use `spdk-sys` FFI bindings for NVMe operations. Key SPDK functions: `spdk_nvme_probe`, `spdk_nvme_ctrlr_process_io_completions`, `spdk_nvme_ns_cmd_read`/`_write`/`_write_zeroes`, `spdk_nvme_ctrlr_alloc_io_qpair`, `spdk_nvme_ctrlr_cmd_abort`, namespace management via admin commands.

**Rationale**: The `spdk-sys` crate already provides bindgen-generated FFI wrappers. Direct SPDK calls are required for zero-copy NVMe access.

**Alternatives considered**:
- Higher-level SPDK Rust wrappers — none exist in the project; building safe wrappers is part of this component's scope.

## R-005: Asynchronous Operation Tracking

**Decision**: Use a monotonically incrementing `u64` counter as operation handles. Maintain a `HashMap<u64, PendingOp>` in the actor handler to track in-flight async operations with their timeout deadlines and client callback channel references.

**Rationale**: Component-assigned handles guarantee uniqueness (per clarification Q1). A `u64` counter is simple, fast, and cannot collide within practical lifetime.

**Alternatives considered**:
- UUID — unnecessarily expensive for in-process identification.
- Client-supplied IDs — rejected per clarification decision.

## R-006: Timeout Enforcement

**Decision**: Use `std::time::Instant` for deadline tracking. On each poll cycle, check `Instant::now()` against stored deadlines. Timed-out operations get error callbacks.

**Rationale**: `Instant` is monotonic and lightweight. Since the actor already polls in a loop, deadline checks add negligible overhead.

**Alternatives considered**:
- Timer wheels — over-engineered for the expected operation count per poll cycle.
- Tokio timers — rejected (no async runtime).

## R-007: NVMe IO Queue Exploitation

**Decision**: Maintain a pool of IO queue pairs (`spdk_nvme_qpair`) with different depths. Select queue pair based on batch size: small batches use shallow queues (lower latency), large batches use deep queues (higher throughput).

**Rationale**: The design explicitly requires exploiting different IO queues with different depths to minimize latency for a given batch size. The selection heuristic can be tuned via benchmarks.

**Alternatives considered**:
- Single queue pair — rejected per design requirement.
- Per-client queue pairs — considered but not required; queue selection based on batch size is the design intent.

## R-008: Telemetry Implementation

**Decision**: Gate telemetry collection behind `#[cfg(feature = "telemetry")]`. Use atomic counters for operation counts and `f64` min/max/mean tracking with compare-and-swap. Expose via `IBlockDevice` trait methods.

**Rationale**: Cargo features are the standard mechanism for conditional compilation. Atomics avoid locking overhead on the hot path.

**Alternatives considered**:
- Always-on with runtime toggle — rejected per design requirement that the feature flag controls compilation.
- `metrics` crate — unnecessary dependency for in-process statistics.

## R-009: define_component! Macro Usage

**Decision**: Use the `define_component!` macro (from `component-macros`) to define `BlockDeviceSpdkNvmeComponent` with `provides: [IBlockDevice]` and `receptacles: { logger: ILogger, spdk_env: ISPDKEnv }`.

**Rationale**: Follows the established pattern in `spdk-env/src/lib.rs` where `SPDKEnvComponent` is defined with `define_component!`. This macro generates `IUnknown` implementation, receptacle fields, and constructor.

**Alternatives considered**:
- Manual `IUnknown` implementation — rejected because the macro handles boilerplate correctly and consistently.

# Research: Channel Backend Benchmarks

**Feature**: 004-channel-benchmarks
**Date**: 2026-03-31

## R1: Third-Party Channel Crate Selection

### Decision: Use crossbeam-channel, kanal, rtrb, tokio::sync::mpsc

### Rationale

Each crate targets a different niche in the channel design space:

| Crate | Version | License | Topology | Key Design |
|-------|---------|---------|----------|------------|
| crossbeam-channel | 0.5.x | MIT/Apache-2.0 | SPSC, MPSC, MPMC | Lock-free bounded & unbounded; go-style select |
| kanal | 0.1.x | MIT | SPSC, MPSC, MPMC | High-performance bounded; low-latency design |
| rtrb | 0.3.x | MIT/Apache-2.0 | SPSC only | Real-time safe, wait-free, cache-friendly ring buffer |
| tokio (sync feature) | 1.x | MIT | MPSC | Async-first but provides `blocking_send`/`blocking_recv` |

### Alternatives Considered

- **flume**: Good MPSC channel but overlaps significantly with crossbeam-channel. Excluded to limit scope.
- **ringbuf**: Another SPSC ring buffer but less maintained than rtrb.
- **async-channel**: Async-first, but less established than tokio for sync use.

## R2: Tokio Sync-Only Usage

### Decision: Use tokio with `features = ["sync"]` only; no async runtime required

### Rationale

Tokio's `mpsc::channel` provides `blocking_send()` and `blocking_recv()` methods that work without a tokio runtime. This allows fair benchmark comparison with other synchronous channel backends. The `sync` feature flag is minimal and does not pull in the full tokio runtime.

### Implementation Note

```rust
// No runtime needed:
let (tx, mut rx) = tokio::sync::mpsc::channel::<u64>(capacity);
tx.blocking_send(42).unwrap();
let val = rx.blocking_recv().unwrap();
```

## R3: Adapter Pattern for IUnknown Integration

### Decision: Each backend gets a wrapper struct implementing IUnknown with ISender/IReceiver

### Rationale

The existing pattern (SpscChannel, MpscChannel) uses:
- OnceLock for lazy interface creation
- AtomicBool CAS for binding enforcement
- InterfaceInfo vec for introspection

Each new backend follows the same pattern:
1. Wrapper struct holds the third-party channel + OnceLock fields + interface_info
2. Inner sender/receiver types implement ISender/IReceiver by delegating to the third-party API
3. IUnknown impl uses OnceLock + CAS (same pattern as SpscChannel/MpscChannel)

### Key Mapping

| Backend | ISender wraps | IReceiver wraps | Topology |
|---------|---------------|-----------------|----------|
| CrossbeamBoundedChannel | crossbeam_channel::Sender | crossbeam_channel::Receiver | SPSC + MPSC |
| CrossbeamUnboundedChannel | crossbeam_channel::Sender | crossbeam_channel::Receiver | SPSC + MPSC |
| KanalChannel | kanal::Sender | kanal::Receiver | SPSC + MPSC |
| RtrbChannel | rtrb::Producer | rtrb::Consumer | SPSC only |
| TokioMpscChannel | tokio::sync::mpsc::Sender | tokio::sync::mpsc::Receiver | MPSC only |

## R4: Binding Enforcement Strategy

### Decision: Reuse AtomicBool CAS pattern from existing channels

### Rationale

- **SPSC backends** (rtrb): CAS on both sender_bound and receiver_bound. Second query returns None.
- **MPSC backends** (crossbeam, kanal, tokio): CAS on receiver_bound only. ISender query always succeeds (sender is Clone).
- **Crossbeam bounded/unbounded**: Support both SPSC and MPSC. Sender is Clone, so treat as MPSC for binding (multiple senders OK, single receiver enforced).

## R5: Benchmark Design

### Decision: Criterion benchmark groups organized by topology with parameterized configurations

### Rationale

Criterion's `BenchmarkGroup` with `bench_with_input` enables parameterized benchmarks. Each group compares all compatible backends under identical conditions.

### Benchmark Matrix

| Dimension | Values |
|-----------|--------|
| Topology | SPSC, MPSC |
| Message size | Small (u64 = 8 bytes), Large (Vec<u8> = 1024 bytes) |
| Queue capacity | 64, 1024, 16384 |
| Producer count (MPSC) | 2, 4, 8 |
| Message count | 100,000 per benchmark iteration |

### SPSC Backends
- Built-in SpscChannel
- Crossbeam bounded
- Crossbeam unbounded
- Kanal
- rtrb

### MPSC Backends
- Built-in MpscChannel
- Crossbeam bounded
- Crossbeam unbounded
- Kanal
- Tokio MPSC

## R6: Error Mapping

### Decision: Map third-party errors to existing ChannelError variants

| Third-party error | Maps to |
|-------------------|---------|
| crossbeam SendError / RecvError | ChannelError::Closed |
| crossbeam TrySendError::Full | ChannelError::Full |
| crossbeam TryRecvError::Empty | ChannelError::Empty |
| kanal SendError / ReceiveError | ChannelError::Closed |
| rtrb PushError::Full | ChannelError::Full |
| rtrb PopError | ChannelError::Empty |
| tokio SendError | ChannelError::Closed |
| tokio TrySendError::Full | ChannelError::Full |

## R7: rtrb Special Handling

### Decision: rtrb wrapper holds Producer/Consumer in Mutex because they are !Sync

### Rationale

rtrb's `Producer` and `Consumer` are `Send` but not `Sync` (they use internal non-atomic state for wait-free operation). Since ISender/IReceiver require `Send + Sync`, we wrap them in `Mutex<Producer<T>>` and `Mutex<Consumer<T>>`. The Mutex overhead is acceptable because:
1. SPSC means only one thread accesses each endpoint
2. The Mutex is uncontended in practice
3. Benchmarks will show the real-world cost of this adaptation

## R8: Crossbeam Unbounded Back-Pressure

### Decision: Unbounded channels have no capacity parameter; document that benchmark results are not directly comparable to bounded channels for back-pressure scenarios

### Rationale

Crossbeam unbounded channels never block on send (they grow dynamically). This means:
- `try_send` always succeeds (never returns Full)
- Throughput benchmarks may show higher numbers but at the cost of memory
- Back-pressure benchmarks are not meaningful for unbounded channels

The benchmark suite will include unbounded results with clear annotations.

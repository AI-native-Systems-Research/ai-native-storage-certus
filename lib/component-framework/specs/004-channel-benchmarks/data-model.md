# Data Model: Channel Backend Benchmarks

**Feature**: 004-channel-benchmarks
**Date**: 2026-03-31

## Entities

### Channel Backend Components

Each third-party channel backend is a Rust struct implementing `IUnknown`, providing `ISender<T>` and/or `IReceiver<T>` interfaces.

| Entity | Module | Topology | Bounded | Dependencies |
|--------|--------|----------|---------|--------------|
| `CrossbeamBoundedChannel<T>` | `channel::crossbeam_bounded` | SPSC + MPSC | Yes | crossbeam-channel |
| `CrossbeamUnboundedChannel<T>` | `channel::crossbeam_unbounded` | SPSC + MPSC | No | crossbeam-channel |
| `KanalChannel<T>` | `channel::kanal_bounded` | SPSC + MPSC | Yes | kanal |
| `RtrbChannel<T>` | `channel::rtrb_spsc` | SPSC only | Yes | rtrb |
| `TokioMpscChannel<T>` | `channel::tokio_mpsc` | MPSC only | Yes | tokio (sync) |

### Inner Sender/Receiver Wrappers

Each backend has inner types that implement `ISender<T>` / `IReceiver<T>`:

| Wrapper | Wraps | Implements | Notes |
|---------|-------|------------|-------|
| `CrossbeamSender<T>` | `crossbeam_channel::Sender<T>` | `ISender<T>` | Clone for MPSC |
| `CrossbeamReceiver<T>` | `crossbeam_channel::Receiver<T>` | `IReceiver<T>` | — |
| `KanalSender<T>` | `kanal::Sender<T>` | `ISender<T>` | Clone for MPSC |
| `KanalReceiver<T>` | `kanal::Receiver<T>` | `IReceiver<T>` | — |
| `RtrbSender<T>` | `Mutex<rtrb::Producer<T>>` | `ISender<T>` | Mutex because Producer is !Sync |
| `RtrbReceiver<T>` | `Mutex<rtrb::Consumer<T>>` | `IReceiver<T>` | Mutex because Consumer is !Sync |
| `TokioSender<T>` | `tokio::sync::mpsc::Sender<T>` | `ISender<T>` | Clone for MPSC |
| `TokioReceiver<T>` | `Mutex<tokio::sync::mpsc::Receiver<T>>` | `IReceiver<T>` | Mutex because Receiver is !Sync |

### Common Fields per Channel Component

All channel component structs share this pattern:

```text
Fields:
  - sender_bound: Arc<AtomicBool>       // SPSC enforcement (CAS)
  - receiver_bound: Arc<AtomicBool>     // Single-receiver enforcement (CAS)
  - sender_iface: OnceLock<Box<dyn Any + Send + Sync>>   // Lazy ISender
  - receiver_iface: OnceLock<Box<dyn Any + Send + Sync>> // Lazy IReceiver
  - interface_info: Vec<InterfaceInfo>  // Introspection metadata
  + backend-specific channel handle
```

### Benchmark Configuration

```text
BenchmarkConfig:
  - message_count: usize      (default: 100_000)
  - message_size: MessageSize  (Small = u64, Large = Vec<u8> 1024 bytes)
  - queue_capacity: usize      (64, 1024, 16384)
  - producer_count: usize      (1 for SPSC; 2, 4, 8 for MPSC)
```

## Relationships

```text
ISender<T> <|-- CrossbeamSender<T>
ISender<T> <|-- KanalSender<T>
ISender<T> <|-- RtrbSender<T>
ISender<T> <|-- TokioSender<T>
ISender<T> <|-- Sender<T>           (existing)
ISender<T> <|-- MpscSender<T>       (existing)

IReceiver<T> <|-- CrossbeamReceiver<T>
IReceiver<T> <|-- KanalReceiver<T>
IReceiver<T> <|-- RtrbReceiver<T>
IReceiver<T> <|-- TokioReceiver<T>
IReceiver<T> <|-- Receiver<T>       (existing)

IUnknown <|-- CrossbeamBoundedChannel<T>
IUnknown <|-- CrossbeamUnboundedChannel<T>
IUnknown <|-- KanalChannel<T>
IUnknown <|-- RtrbChannel<T>
IUnknown <|-- TokioMpscChannel<T>
IUnknown <|-- SpscChannel<T>        (existing)
IUnknown <|-- MpscChannel<T>        (existing)
```

## Validation Rules

- **Capacity**: Must be > 0 for bounded channels. rtrb requires power-of-two capacity (same as built-in).
- **Binding enforcement**: SPSC backends reject second sender AND second receiver. MPSC backends reject only second receiver.
- **Message type**: `T: Send + 'static` (same constraint as existing channels).
- **Closure semantics**: When all senders drop, receiver gets `ChannelError::Closed` after draining. Same as existing channels.

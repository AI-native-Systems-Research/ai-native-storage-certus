# Contract: Channel Backend Component

**Feature**: 004-channel-benchmarks
**Date**: 2026-03-31

## Public API Contract

Every third-party channel backend MUST expose the following public API, consistent with the existing `SpscChannel<T>` and `MpscChannel<T>` patterns.

### Construction

```rust
// Bounded channels
ChannelType::<T>::new(capacity: usize) -> Self
ChannelType::<T>::with_default_capacity() -> Self  // capacity = 1024

// Unbounded channel (CrossbeamUnboundedChannel only)
CrossbeamUnboundedChannel::<T>::new() -> Self
```

- `new()` panics if capacity is 0 (bounded channels)
- rtrb requires power-of-two capacity (panics otherwise)

### Direct API (optional convenience methods)

```rust
// For MPSC-capable backends:
fn sender(&self) -> Result<SenderType<T>, ChannelError>   // Always Ok for MPSC
fn receiver(&self) -> Result<Receiver<T>, ChannelError>    // Err if already bound

// For SPSC-only backends:
fn sender(&self) -> Result<SenderType<T>, ChannelError>    // Err if already bound
fn receiver(&self) -> Result<Receiver<T>, ChannelError>    // Err if already bound
```

### IUnknown Implementation

```rust
impl<T: Send + 'static> IUnknown for ChannelType<T> {
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)>;
    fn version(&self) -> &str;                    // Returns "1.0.0"
    fn provided_interfaces(&self) -> &[InterfaceInfo];
    fn receptacles(&self) -> &[ReceptacleInfo];   // Returns &[]
    fn connect_receptacle_raw(&self, name: &str, provider: &dyn IUnknown)
        -> Result<(), RegistryError>;             // Returns Err (no receptacles)
}
```

### Query Behavior

| Backend | ISender query | IReceiver query |
|---------|---------------|-----------------|
| CrossbeamBoundedChannel | Always succeeds (MPSC) | First succeeds, second returns None |
| CrossbeamUnboundedChannel | Always succeeds (MPSC) | First succeeds, second returns None |
| KanalChannel | Always succeeds (MPSC) | First succeeds, second returns None |
| RtrbChannel | First succeeds, second returns None (SPSC) | First succeeds, second returns None |
| TokioMpscChannel | Always succeeds (MPSC) | First succeeds, second returns None |

### ISender<T> Contract

```rust
fn send(&self, value: T) -> Result<(), ChannelError>;
fn try_send(&self, value: T) -> Result<(), ChannelError>;
```

- `send`: Blocks until space available (bounded) or succeeds immediately (unbounded)
- `try_send`: Returns `ChannelError::Full` if bounded and full, or succeeds immediately (unbounded)
- Both return `ChannelError::Closed` if receiver is dropped

### IReceiver<T> Contract

```rust
fn recv(&self) -> Result<T, ChannelError>;
fn try_recv(&self) -> Result<T, ChannelError>;
```

- `recv`: Blocks until message available; returns `ChannelError::Closed` when all senders dropped and queue empty
- `try_recv`: Returns `ChannelError::Empty` if no message available; `ChannelError::Closed` if closed and empty

### Introspection Contract

```rust
version()              -> "1.0.0"
provided_interfaces()  -> [InterfaceInfo("ISender"), InterfaceInfo("IReceiver")]
receptacles()          -> []
```

### Thread Safety

All channel components and their sender/receiver wrappers MUST be `Send + Sync`.

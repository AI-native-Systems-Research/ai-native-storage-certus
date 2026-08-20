# Public API Contract: Actor Model with Channel Components

**Feature**: 003-actor-channels | **Date**: 2026-03-31

## New Public Types (component-core)

### Actor Module (`crate::actor`)

```rust
/// Error type for actor operations.
pub enum ActorError {
    AlreadyActive,
    NotActive,
    SendFailed(String),
    ShutdownTimeout,
}

/// Trait that users implement to define actor message-handling behavior.
///
/// `M` is the message type. Must be `Send + 'static`.
pub trait ActorHandler<M: Send + 'static>: Send + 'static {
    /// Called for each message received. Runs on the actor's dedicated thread.
    fn handle(&mut self, msg: M);

    /// Called once when the actor starts (before the message loop).
    /// Default: no-op.
    fn on_start(&mut self) {}

    /// Called once when the actor is shutting down (after the message loop exits).
    /// Default: no-op.
    fn on_stop(&mut self) {}
}

/// Handle to a running actor. Returned by `Actor::activate()`.
pub struct ActorHandle<M: Send + 'static> { /* ... */ }

impl<M: Send + 'static> ActorHandle<M> {
    /// Send a message to the actor. Blocks if the inbound channel is full.
    pub fn send(&self, msg: M) -> Result<(), ActorError>;

    /// Try to send without blocking. Returns Err if channel is full.
    pub fn try_send(&self, msg: M) -> Result<(), ActorError>;

    /// Deactivate the actor: signal shutdown, join thread.
    pub fn deactivate(self) -> Result<(), ActorError>;
}

/// An actor component that owns a thread and processes messages sequentially.
pub struct Actor<M, H>
where
    M: Send + 'static,
    H: ActorHandler<M>,
{ /* ... */ }

impl<M, H> Actor<M, H>
where
    M: Send + 'static,
    H: ActorHandler<M>,
{
    /// Create a new actor with the given handler and error callback.
    /// `capacity` sets the inbound channel queue depth (default 1024).
    pub fn new(
        handler: H,
        error_callback: impl Fn(Box<dyn std::any::Any + Send>) + Send + Sync + 'static,
    ) -> Self;

    /// Create with custom channel capacity.
    pub fn with_capacity(
        handler: H,
        capacity: usize,
        error_callback: impl Fn(Box<dyn std::any::Any + Send>) + Send + Sync + 'static,
    ) -> Self;

    /// Activate the actor: spawn its thread and start the message loop.
    /// Returns a handle for sending messages and deactivating.
    pub fn activate(&self) -> Result<ActorHandle<M>, ActorError>;

    /// Check whether the actor is currently running.
    pub fn is_active(&self) -> bool;
}
```

### Channel Module (`crate::channel`)

```rust
/// Error type for channel operations.
pub enum ChannelError {
    Full,
    Empty,
    Closed,
    BindingRejected { reason: String },
}

/// Sender endpoint for typed message passing.
pub struct Sender<T: Send + 'static> { /* ... */ }

impl<T: Send + 'static> Sender<T> {
    /// Send a message. Blocks if the queue is full.
    pub fn send(&self, value: T) -> Result<(), ChannelError>;

    /// Try to send without blocking.
    pub fn try_send(&self, value: T) -> Result<(), ChannelError>;
}

impl<T: Send + 'static> Clone for Sender<T> { /* MPSC: cloneable */ }

/// Receiver endpoint for typed message passing.
pub struct Receiver<T: Send + 'static> { /* ... */ }

impl<T: Send + 'static> Receiver<T> {
    /// Receive a message. Blocks if the queue is empty.
    /// Returns `Err(Closed)` when all senders are dropped and queue is drained.
    pub fn recv(&self) -> Result<T, ChannelError>;

    /// Try to receive without blocking.
    pub fn try_recv(&self) -> Result<T, ChannelError>;
}

/// SPSC channel component. First-class component providing ISender and IReceiver.
pub struct SpscChannel<T: Send + 'static> { /* ... */ }

impl<T: Send + 'static> SpscChannel<T> {
    /// Create a new SPSC channel with the given capacity.
    /// Capacity must be a power of two; panics otherwise.
    pub fn new(capacity: usize) -> Self;

    /// Create with default capacity (1024).
    pub fn with_default_capacity() -> Self;

    /// Get the sender endpoint. Returns Err if already bound.
    pub fn sender(&self) -> Result<Sender<T>, ChannelError>;

    /// Get the receiver endpoint. Returns Err if already bound.
    pub fn receiver(&self) -> Result<Receiver<T>, ChannelError>;
}

/// MPSC channel component. First-class component providing ISender and IReceiver.
pub struct MpscChannel<T: Send + 'static> { /* ... */ }

impl<T: Send + 'static> MpscChannel<T> {
    /// Create a new MPSC channel with the given capacity.
    pub fn new(capacity: usize) -> Self;

    /// Create with default capacity (1024).
    pub fn with_default_capacity() -> Self;

    /// Get a sender endpoint. Can be called multiple times (multi-producer).
    pub fn sender(&self) -> Result<Sender<T>, ChannelError>;

    /// Get the receiver endpoint. Returns Err if already bound.
    pub fn receiver(&self) -> Result<Receiver<T>, ChannelError>;
}
```

### Channel Interfaces (for component binding)

```rust
/// Interface trait for sending messages (provided by channel components).
pub trait ISender: Interface {
    /// Send a type-erased message. Used by the binding system.
    fn send_any(&self, msg: Box<dyn std::any::Any + Send>) -> Result<(), ChannelError>;
}

/// Interface trait for receiving messages (provided by channel components).
pub trait IReceiver: Interface {
    /// Receive a type-erased message. Used by the binding system.
    fn recv_any(&self) -> Result<Box<dyn std::any::Any + Send>, ChannelError>;
}
```

## Extended Types

### error.rs additions

```rust
/// Re-export of ActorError and ChannelError at crate level.
pub use crate::actor::ActorError;
pub use crate::channel::ChannelError;
```

### lib.rs additions

```rust
pub mod actor;
pub mod channel;

pub use actor::{Actor, ActorError, ActorHandle, ActorHandler};
pub use channel::{
    ChannelError, ISender, IReceiver, MpscChannel, Receiver, Sender, SpscChannel,
};
```

## IUnknown Implementation

Both `SpscChannel<T>` and `MpscChannel<T>` implement `IUnknown`:

- `query_interface_raw`: returns `Arc<dyn ISender>` and `Arc<dyn IReceiver>`
- `version`: `"1.0.0"`
- `provided_interfaces`: `["ISender", "IReceiver", "IUnknown"]`
- `receptacles`: `[]` (channels have no receptacles)
- `connect_receptacle_raw`: returns `BindingFailed` (no receptacles)

`Actor<M, H>` delegates `IUnknown` to its inner component (if any) or implements it directly with actor-specific metadata.

## Macro: `define_actor!`

```rust
/// Generates actor boilerplate: IUnknown impl, receptacle wiring, and
/// ActorHandler dispatch. Similar to define_component! but adds
/// activate/deactivate lifecycle.
define_actor! {
    pub MyActor<MyMessage> {
        version: "1.0.0",
        provides: [IMyOutput],
        receptacles: {
            output: ISender,
        },
    }
}
```

The macro generates:
- Struct with `InterfaceMap`, receptacle fields, and actor state
- `IUnknown` implementation
- `new()` constructor
- Integration with `Actor<M, H>` for lifecycle management

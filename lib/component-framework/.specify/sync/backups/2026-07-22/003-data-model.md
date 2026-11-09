# Data Model: Actor Model with Channel Components

**Feature**: 003-actor-channels | **Date**: 2026-03-31

## Entities

### Actor

An `Actor<M>` wraps a component that owns a dedicated thread and processes messages of type `M` sequentially.

| Field | Type | Description |
|-------|------|-------------|
| component | (inner component) | The underlying component (implements IUnknown) |
| thread | Option\<JoinHandle\<()\>\> | The actor's dedicated thread; `Some` when active, `None` when stopped |
| shutdown | Arc\<AtomicBool\> | Shared flag checked by the message loop to exit |
| error_callback | Arc\<dyn Fn(Box\<dyn Any + Send\>) + Send + Sync\> | User-provided callback invoked when the message handler panics |
| state | AtomicU8 | Lifecycle state: Idle(0), Running(1) |

**State transitions**:
- `Idle` → `Running`: via `activate()` — spawns the thread, starts message loop
- `Running` → `Idle`: via `deactivate()` — sets shutdown flag, closes inbound channel sender, joins thread
- `Idle` → `Idle` (deactivate): returns `ActorError::NotActive`
- `Running` → `Running` (activate): returns `ActorError::AlreadyActive`

### ActorHandle

Returned by `activate()`. Provides the external interface to a running actor.

| Field | Type | Description |
|-------|------|-------------|
| sender | Sender\<M\> | Channel sender for delivering messages to the actor |
| shutdown | Arc\<AtomicBool\> | Shared shutdown flag |
| thread | Option\<JoinHandle\<()\>\> | Joined on deactivate/drop |

### Channel (SPSC)

A `SpscChannel<T>` is a first-class component that provides sender and receiver endpoints backed by a lock-free ring buffer.

| Field | Type | Description |
|-------|------|-------------|
| interface_map | InterfaceMap | Standard component interface storage |
| sender_bound | AtomicBool | Whether a sender is currently bound |
| receiver_bound | AtomicBool | Whether a receiver is currently bound |
| queue | Arc\<RingBuffer\<T\>\> | Shared lock-free ring buffer |
| capacity | usize | Queue depth (power of two, default 1024) |

**Provided interfaces**: `ISender<T>`, `IReceiver<T>`, `IUnknown`

**Binding constraints**:
- Max 1 sender (SPSC): `sender_bound` must be `false` to bind
- Max 1 receiver (SPSC): `receiver_bound` must be `false` to bind
- On sender disconnect: `sender_bound` set to `false`, slot available for rebinding

### Channel (MPSC)

A `MpscChannel<T>` is similar but allows multiple senders.

| Field | Type | Description |
|-------|------|-------------|
| interface_map | InterfaceMap | Standard component interface storage |
| sender_count | AtomicUsize | Number of currently bound senders |
| receiver_bound | AtomicBool | Whether a receiver is currently bound |
| queue | Arc\<MpscRingBuffer\<T\>\> | Shared MPSC lock-free ring buffer |
| capacity | usize | Queue depth (power of two, default 1024) |

**Provided interfaces**: `ISender<T>`, `IReceiver<T>`, `IUnknown`

**Binding constraints**:
- Multiple senders allowed: `sender_count` incremented on each bind
- Max 1 receiver: `receiver_bound` must be `false` to bind
- On sender disconnect: `sender_count` decremented; when reaches 0, receiver gets closed signal

### RingBuffer (SPSC core)

Lock-free bounded queue for single-producer, single-consumer.

| Field | Type | Description |
|-------|------|-------------|
| buffer | Box\<[UnsafeCell\<MaybeUninit\<T\>\>]\> | Heap-allocated slot array |
| capacity | usize | Must be power of two |
| mask | usize | `capacity - 1` for bitwise index wrapping |
| head | CachePadded\<AtomicUsize\> | Consumer read position |
| tail | CachePadded\<AtomicUsize\> | Producer write position |
| sender_alive | AtomicBool | `false` when sender disconnected (signals closure) |

**Invariants**:
- `tail - head <= capacity` (never more items than capacity)
- `head <= tail` (head never passes tail)
- Only producer writes to `tail`; only consumer writes to `head`
- Slots between `head` and `tail` contain initialized values

### Sender\<T\>

Typed sender endpoint cloned from a channel.

| Field | Type | Description |
|-------|------|-------------|
| queue | Arc\<RingBuffer\<T\>\> (or Arc\<MpscRingBuffer\<T\>\>) | Shared queue reference |
| _marker | PhantomData | Lifetime/ownership marker |

### Receiver\<T\>

Typed receiver endpoint from a channel.

| Field | Type | Description |
|-------|------|-------------|
| queue | Arc\<RingBuffer\<T\>\> (or Arc\<MpscRingBuffer\<T\>\>) | Shared queue reference |
| _marker | PhantomData | Lifetime/ownership marker |

## Error Types

### ActorError

| Variant | Description |
|---------|-------------|
| AlreadyActive | `activate()` called on a running actor |
| NotActive | `deactivate()` called on an idle actor |
| SendFailed | Failed to send message to actor's inbound channel |
| ShutdownTimeout | Thread join timed out during deactivation |

### ChannelError

| Variant | Description |
|---------|-------------|
| Full | Queue is full (for `try_send`) |
| Empty | Queue is empty (for `try_recv`) |
| Closed | All senders disconnected; no more messages will arrive |
| BindingRejected { reason: String } | Topology constraint violated (e.g., second sender on SPSC) |

## Relationships

```text
Actor<M> ──owns──> thread (JoinHandle)
Actor<M> ──has──> inbound Receiver<M> (receptacle bound to a channel)
Actor<M> ──has──> outbound Sender<M> (receptacle bound to a channel, optional)

SpscChannel<T> ──provides──> ISender<T> (max 1 binding)
SpscChannel<T> ──provides──> IReceiver<T> (max 1 binding)
SpscChannel<T> ──owns──> RingBuffer<T>

MpscChannel<T> ──provides──> ISender<T> (unlimited bindings)
MpscChannel<T> ──provides──> IReceiver<T> (max 1 binding)
MpscChannel<T> ──owns──> MpscRingBuffer<T>

Sender<T> ──refs──> RingBuffer<T> (via Arc)
Receiver<T> ──refs──> RingBuffer<T> (via Arc)
```

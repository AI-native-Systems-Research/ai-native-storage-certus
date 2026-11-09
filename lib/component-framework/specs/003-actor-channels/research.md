# Research: Actor Model with Channel Components

**Feature**: 003-actor-channels | **Date**: 2026-03-31

## Lock-Free Ring Buffer (SPSC Core)

**Decision**: Build a bounded, power-of-two SPSC ring buffer using `AtomicUsize` head/tail pointers, `UnsafeCell<MaybeUninit<T>>` slots, and cache-line padding to prevent false sharing.

**Rationale**: The project constitution mandates minimal dependencies (no external lock-free queue crate). A bounded ring buffer with atomic head/tail is the simplest correct SPSC queue — well-understood, zero-allocation on the hot path, and does not require a CAS loop (single writer per pointer). Power-of-two capacity enables bitwise masking instead of modulo.

**Alternatives considered**:
- **crossbeam-channel**: Mature, battle-tested, but introduces a large dependency and violates the constitution's minimal-dependency principle.
- **Linked-list queue**: Unbounded, but requires per-message allocation and pointer chasing (poor cache behavior, slower throughput).
- **Mutex-protected `VecDeque`**: Simplest to implement but violates FR-010 (lock-free requirement) and fails performance goals.

**Key implementation details**:
- Slots: `Box<[UnsafeCell<MaybeUninit<T>>]>` — heap-allocated, fixed at creation.
- Head/tail: `AtomicUsize` with `Ordering::Acquire` on load, `Ordering::Release` on store. No need for `SeqCst`.
- Capacity check: assert power-of-two at construction; mask = capacity - 1.
- Full: `(tail - head) == capacity` → sender blocks (spin-then-park via `thread::park`/`unpark`).
- Empty: `head == tail` → receiver blocks similarly.
- `unsafe` justification: UnsafeCell gives interior mutability for concurrent write (producer) and read (consumer) to *different* slots, which is safe because head/tail atomics establish the happens-before relationship.

## MPSC Extension Strategy

**Decision**: Wrap the SPSC core with an `AtomicUsize`-based sender ticket for serialized writes. Each sender atomically claims a slot index, writes its value, then publishes. A separate "committed" bitmap or sequential commit counter ensures the receiver only reads fully-written slots.

**Rationale**: Building MPSC atop the SPSC core maximizes code reuse and keeps the queue logic in one place. The ticket approach avoids a CAS loop on the tail pointer (each sender gets a unique slot), maintaining near-lock-free throughput under moderate contention.

**Alternatives considered**:
- **Separate MPSC queue from scratch**: Would duplicate ring buffer logic.
- **Lock-per-sender**: Simple but defeats lock-free requirement.
- **Multiple SPSC queues merged**: Adds complexity for the receiver (must poll N queues) and breaks message ordering guarantees across senders.

## Actor Threading Model

**Decision**: One OS thread per actor, spawned on `activate()`, joined on `deactivate()`. The actor's thread runs a message loop: receive from inbound channel → dispatch to handler → loop. A poison-pill sentinel or atomic flag signals shutdown.

**Rationale**: Per-spec assumption — "Actors use one OS thread per actor." The simplest correct approach: `std::thread::spawn` with a loop that checks a shutdown flag after each receive. No async runtime, no thread pool, no external dependencies.

**Alternatives considered**:
- **Async runtime (tokio/async-std)**: Over-scoped — spec explicitly excludes async and thread pooling.
- **Thread pool with work-stealing**: More efficient for many actors but not in scope.
- **Green threads / coroutines**: Requires nightly features or external crate.

**Shutdown protocol**:
1. `deactivate()` sets `AtomicBool` shutdown flag.
2. If the actor is blocked on receive, the channel is closed (sender side dropped or explicit close), unblocking the receive.
3. Actor thread checks flag, exits loop, thread is joined.
4. Bounded timeout on join (e.g., 5 seconds) to avoid hanging.

## Actor Panic Recovery

**Decision**: Wrap the message handler invocation in `std::panic::catch_unwind`. On panic, invoke the user-supplied error callback with the panic payload, then continue the message loop.

**Rationale**: Per FR-006, actor handler panics must not crash the host process. `catch_unwind` is the standard Rust mechanism. The handler must be `UnwindSafe` (or we use `AssertUnwindSafe` — justified because we're catching and recovering, not relying on data consistency across the panic boundary).

**Key detail**: After a panic, the actor's internal state may be inconsistent. The error callback should log/alert, and the actor continues processing subsequent messages. If the user wants the actor to stop on panic, the callback can trigger deactivation externally.

## Channel-as-Component Integration

**Decision**: Channel components implement `IUnknown` (via `define_component!` or manual impl) and provide two interfaces: `ISender<T>` and `IReceiver<T>`. Binding enforcement (SPSC/MPSC topology) is implemented via a custom `connect_receptacle_raw` that checks an atomic connection counter.

**Rationale**: The spec requires channels to be first-class components discoverable via introspection and bindable via the existing registry. Using the same `IUnknown` trait means channels integrate seamlessly with `ComponentRegistry`, `bind()`, and `ComponentRef`.

**Design detail**: Since `ISender<T>` and `IReceiver<T>` are generic, but `IUnknown::query_interface_raw` is object-safe (uses `TypeId`), the channel must be monomorphized for a specific message type `T` at creation time. Each `SpscChannel<T>` or `MpscChannel<T>` is a concrete component type. The `define_interface!` macro can generate the sender/receiver interfaces for specific `T` types, or we define non-generic `ISender`/`IReceiver` traits that use `Box<dyn Any + Send>` for type-erased message passing, with a typed wrapper on top.

**Chosen approach**: Non-generic `ISender`/`IReceiver` traits at the component boundary (for IUnknown compatibility), with a typed `Sender<T>`/`Receiver<T>` wrapper that handles serialization. This keeps the component model uniform while providing type safety to the user.

## Blocking vs Non-Blocking Send/Receive

**Decision**: Default behavior is blocking (per spec). When the queue is full, the sender blocks via `thread::park()` / `thread::unpark()` signaling. When empty, the receiver blocks similarly. Non-blocking `try_send`/`try_recv` variants return `Err` immediately.

**Rationale**: Spec edge cases state "the sender blocks until space becomes available." Thread parking is lighter than condvar for the SPSC case (no mutex needed). For MPSC, a single `Condvar` may be needed for the receiver since multiple senders could `unpark` simultaneously.

## Sender Slot Release on Disconnect

**Decision**: Track active sender count via `AtomicUsize`. When a sender is dropped, decrement the counter. When it reaches zero, signal channel closure to the receiver. For SPSC, the counter is simply 0 or 1. Disconnection makes the slot available for rebinding (FR-017).

**Rationale**: The spec requires that when all senders disconnect, the receiver gets a "closed" signal, and that SPSC sender slots become available for rebinding after disconnect.

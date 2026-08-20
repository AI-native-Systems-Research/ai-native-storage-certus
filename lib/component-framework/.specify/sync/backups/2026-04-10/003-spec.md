# Feature Specification: Actor Model with Channel Components

**Feature Branch**: `003-actor-channels`
**Created**: 2026-03-31
**Status**: Draft
**Input**: User description: "In addition to plain components, the framework should support an Actor-based model whereby components own their own threads and can exchange messages via channels. Actors should use the basic component, interfaces and receptacles paradigm. Channels must be components themselves, as first-class entities, which are bound accordingly. Default channel implementations include shared memory with lock-free queues. At least SPSC and MPSC channels are provided. Channel binds should be restricted depending on whether they inherently support single (e.g., SPSC) or multiple (e.g. MPSC) bindings. Provide examples of using actor components."

## User Scenarios & Testing

### User Story 1 - Actor Component Lifecycle (Priority: P1)

A framework user creates an actor component that owns its own thread and processes messages sequentially. The actor starts when activated, processes incoming messages one at a time, and stops cleanly when deactivated. Actors use the same component, interface, and receptacle model as plain components, so existing framework knowledge transfers directly.

**Why this priority**: Without a working actor lifecycle, no other actor features are possible. This is the foundational building block.

**Independent Test**: Create a single actor, send it a message, verify it processes the message on its own thread (different from the caller's thread), and verify clean shutdown with no resource leaks.

**Acceptance Scenarios**:

1. **Given** an actor component defined using the framework's component model, **When** the actor is activated, **Then** it begins running on its own dedicated thread and is ready to receive messages.
2. **Given** a running actor, **When** the actor is deactivated, **Then** it finishes processing any in-progress message, stops its thread, and releases all resources.
3. **Given** a running actor, **When** multiple messages arrive, **Then** messages are processed one at a time in the order they were received.
4. **Given** an actor component, **When** inspected via the framework's introspection (provided interfaces, receptacles, version), **Then** it reports the same metadata as any other component.

---

### User Story 2 - Channel Components (SPSC and MPSC) (Priority: P1)

A framework user connects two or more actors using channel components. Channels are first-class components that provide sender and receiver interfaces. The framework provides at least two channel types: single-producer single-consumer (SPSC) and multi-producer single-consumer (MPSC). Channels use shared-memory lock-free queues for high-throughput message passing.

**Why this priority**: Channels are the communication mechanism between actors. Without channels, actors cannot exchange messages.

**Independent Test**: Create an SPSC channel component, bind its sender interface to one actor's outbound receptacle and its receiver interface to another actor's inbound receptacle, send messages, and verify delivery. Repeat with MPSC channel and multiple senders.

**Acceptance Scenarios**:

1. **Given** an SPSC channel component, **When** a single producer sends a message and a single consumer reads, **Then** the message is delivered correctly and in order.
2. **Given** an MPSC channel component, **When** multiple producers send messages concurrently, **Then** all messages are delivered to the single consumer without loss.
3. **Given** a channel component, **When** inspected via framework introspection, **Then** it reports its provided interfaces (sender, receiver) and version like any other component.
4. **Given** an SPSC channel, **When** a second producer attempts to bind to the sender interface, **Then** the binding is rejected with a clear error indicating the channel only supports a single producer.
5. **Given** an MPSC channel, **When** multiple producers bind to the sender interface, **Then** all bindings succeed.

---

### User Story 3 - Binding Enforcement for Channel Topology (Priority: P2)

The framework enforces channel binding constraints at bind time. SPSC channels reject a second sender or second receiver binding. MPSC channels allow multiple senders but reject a second receiver binding. These constraints prevent topology errors before any messages flow, providing fail-fast behavior.

**Why this priority**: Without binding enforcement, users could create invalid topologies (e.g., two receivers on SPSC) leading to data races or lost messages. This is a safety feature that builds on US1 and US2.

**Independent Test**: Attempt to over-bind an SPSC channel (two senders or two receivers) and verify rejection. Attempt to bind multiple senders to an MPSC channel and verify acceptance. Attempt to bind two receivers to an MPSC channel and verify rejection.

**Acceptance Scenarios**:

1. **Given** an SPSC channel with one sender already bound, **When** a second sender attempts to bind, **Then** the bind fails with an error describing the single-producer constraint.
2. **Given** an SPSC channel with one receiver already bound, **When** a second receiver attempts to bind, **Then** the bind fails with an error describing the single-consumer constraint.
3. **Given** an MPSC channel with one sender already bound, **When** a second sender binds, **Then** the bind succeeds.
4. **Given** an MPSC channel with one receiver already bound, **When** a second receiver attempts to bind, **Then** the bind fails with an error describing the single-consumer constraint.
5. **Given** a channel with active bindings, **When** a bound sender disconnects, **Then** the slot becomes available for a new sender to bind.

---

### User Story 4 - Actor-to-Actor Communication Pipeline (Priority: P2)

A framework user assembles a multi-stage processing pipeline where actors communicate through channel components. The assembly can be done using either first-party binding (direct typed wiring) or third-party binding (string-name-based wiring via the registry). This demonstrates the full actor + channel system working end-to-end.

**Why this priority**: This validates the integration of actors, channels, and the existing binding/registry infrastructure. It depends on US1-US3 being complete.

**Independent Test**: Register actor and channel factories in the registry, create a 3-stage pipeline (producer actor -> channel -> processor actor -> channel -> consumer actor), send messages through, and verify correct end-to-end processing.

**Acceptance Scenarios**:

1. **Given** two actor components and a channel, **When** wired via first-party binding, **Then** messages flow from sender actor through channel to receiver actor.
2. **Given** actor and channel factories registered in the component registry, **When** an assembler creates and wires them using third-party binding (string names), **Then** the pipeline functions identically to first-party wiring.
3. **Given** a 3-stage pipeline (producer -> processor -> consumer), **When** the producer sends a sequence of messages, **Then** the consumer receives all messages transformed by the processor, in order.
4. **Given** a channel and an actor, **When** the developer uses the `pipe()` helper to connect them, **Then** messages flow from the channel to the actor without manual forwarder thread code, and the actor is deactivated when the channel closes.

---

### User Story 5 - Actor Component Examples (Priority: P3)

The framework includes runnable examples demonstrating actor components. Examples cover: a simple ping-pong pair, a producer-consumer pipeline, and a fan-in pattern using MPSC. Examples are self-contained, well-commented, and serve as learning material.

**Why this priority**: Examples are documentation and validation, but the framework itself must work first.

**Independent Test**: Each example compiles, runs without errors, and produces expected output.

**Acceptance Scenarios**:

1. **Given** the ping-pong example, **When** run, **Then** two actors exchange a configurable number of messages and print the result.
2. **Given** the producer-consumer example, **When** run, **Then** a producer sends items through an SPSC channel to a consumer that processes them.
3. **Given** the fan-in example, **When** run, **Then** multiple producer actors send to a single consumer through an MPSC channel and all messages are received.
4. **Given** the tokio ping-pong example, **When** run, **Then** two tokio tasks exchange messages through TokioMpscChannel components queried via IUnknown, and all pong replies are received in order.

---

### Edge Cases

- What happens when a message is sent to a channel after the receiver actor has been deactivated? The channel accepts the message regardless of receiver state (up to capacity, then the sender blocks). A future consumer can retrieve queued messages.
- What happens when an actor's message handler panics? The framework catches the panic and reports it as an error. The actor remains alive and continues processing subsequent messages.
- What happens when a channel's internal queue is full? The sender blocks until space becomes available. This is the default backpressure behavior.
- What happens when all senders disconnect from a channel? The receiver gets a "closed" signal after draining remaining queued messages.
- What happens when an actor is deactivated while messages are queued in its inbound channel? The actor finishes its current message, then stops. Remaining messages stay in the channel.
- What happens when activate is called on an already-active actor? An error is returned. Same for deactivate on an already-stopped actor.

## Requirements

### Functional Requirements

**Actor Components**:

- **FR-001**: The framework MUST provide a way to define actor components that own a dedicated thread.
- **FR-002**: Actor components MUST conform to the same component, interface,
  and receptacle model as plain components — implementing `IUnknown` with
  query, version, provided_interfaces, and receptacles methods. Actors MAY
  implement `IUnknown` directly rather than through `define_component!` when
  generics or other language features require it. The observable behavior
  (introspection, interface query, third-party binding compatibility) MUST
  be identical to macro-generated components.
- **FR-003**: An actor MUST process messages sequentially (one at a time) on its own thread.
- **FR-004**: An actor MUST support explicit activation (start thread) and deactivation (stop thread with graceful shutdown). Calling activate on an already-active actor MUST return an error. Double-deactivation MUST be prevented — either by returning a runtime error or by using type-level enforcement (e.g., consuming the handle on deactivation so that a second call is impossible at compile time).
- **FR-005**: An actor MUST be discoverable via the existing introspection mechanism (provided interfaces, receptacles, version).
- **FR-006**: An actor's message handler panic MUST be caught by the framework and MUST NOT crash the host process. The panic MUST be reported via a user-provided error callback set at actor creation time. The actor MUST remain alive and continue processing subsequent messages after a panic.

**Channel Components**:

- **FR-007**: Channels MUST be first-class components with their own provided interfaces (sender endpoint, receiver endpoint).
- **FR-008**: The framework MUST provide at least an SPSC (single-producer, single-consumer) channel implementation.
- **FR-009**: The framework MUST provide at least an MPSC (multi-producer, single-consumer) channel implementation.
- **FR-010**: Channel implementations MUST use shared-memory lock-free queues for message passing.
- **FR-011**: Channels MUST support typed messages — the message type is determined at channel creation or by the channel's interface definition.
- **FR-012**: Channels MUST signal closure to receivers when all senders disconnect.

**Binding Enforcement**:

- **FR-013**: SPSC channels MUST reject a second sender binding with a descriptive error.
- **FR-014**: SPSC channels MUST reject a second receiver binding with a descriptive error.
- **FR-015**: MPSC channels MUST accept multiple sender bindings.
- **FR-016**: MPSC channels MUST reject a second receiver binding with a descriptive error.
- **FR-017**: When a sender disconnects from a channel, the slot MUST become available for a new sender to bind (for SPSC channels).

**Integration**:

- **FR-018**: Actor and channel components MUST be registerable in the existing component registry.
- **FR-019**: Actor and channel components MUST support both first-party and third-party binding.
- **FR-020**: The framework MUST provide a way to configure channel capacity (queue depth) at creation time.

**Examples**:

- **FR-021**: The framework MUST include a runnable ping-pong actor example.
- **FR-022**: The framework MUST include a runnable producer-consumer pipeline example.
- **FR-023**: The framework MUST include a runnable fan-in (MPSC) example.
- **FR-024**: The framework MUST include a runnable tokio ping-pong example demonstrating TokioMpscChannel usage with ISender/IReceiver queried via IUnknown and `tokio::task::spawn_blocking`.

**Ergonomic Helpers**:

- **FR-025**: The framework MUST provide a `pipe()` helper that spawns a forwarder thread bridging a channel `Receiver` to an `ActorHandle`. When the channel closes, the forwarder MUST deactivate the actor and exit. An MPSC variant (`pipe_mpsc()`) MUST also be provided.
- **FR-026**: The framework MUST provide a simplified actor constructor (`Actor::simple()`) that uses default channel capacity (1024) and silently catches panics, for use cases where custom error handling is not needed.
- **FR-027**: Channel components MUST provide a `split()` method that returns both sender and receiver endpoints as a tuple in a single call.

### Key Entities

- **Actor**: A component that owns a dedicated thread and processes messages sequentially. Extends the basic component model with activation/deactivation lifecycle and a message handler.
- **Channel**: A first-class component that provides sender and receiver interfaces for typed message passing. Internally uses a lock-free queue. Enforces topology constraints (SPSC vs MPSC) at bind time.
- **Sender Endpoint**: An interface provided by a channel component that allows a producer to send messages into the channel.
- **Receiver Endpoint**: An interface provided by a channel component that allows a consumer to read messages from the channel.
- **Message**: A typed unit of data passed between actors through channels. Must be sendable across threads.

## Success Criteria

### Measurable Outcomes

- **SC-001**: A user can define, activate, send messages to, and deactivate an actor component in under 10 lines of framework-specific code (excluding boilerplate imports and interface definitions).
- **SC-002**: All channel operations (send, receive) complete without data loss when tested with 100,000 sequential messages.
- **SC-003**: MPSC channel correctly delivers all messages from 8 concurrent producers sending 10,000 messages each (80,000 total) to a single consumer with zero loss.
- **SC-004**: Invalid binding attempts (e.g., second sender on SPSC) are rejected at bind time, not at message-send time.
- **SC-005**: Actor shutdown completes within a bounded time (no hung threads) after deactivation is requested.
- **SC-006**: All three examples (ping-pong, producer-consumer, fan-in) compile, run, and produce correct output.
- **SC-007**: Existing plain-component tests (from features 001 and 002) continue to pass unchanged — full backward compatibility.

## Assumptions

- Actors use one OS thread per actor. Thread pooling or async runtimes are out of scope for this feature.
- Messages are owned values that are moved (not shared) through channels. The message type must be sendable across threads.
- Channel capacity (queue depth) is finite and configurable. The default capacity is 1024 elements. The default behavior when the queue is full is that the sender blocks until space is available.
- The lock-free queue implementation will be built in-house (no external dependencies beyond what the project already uses), consistent with the project constitution's minimal-dependency approach.
- "Deactivation" means the actor finishes its current message, drains no further messages, and joins its thread. Queued messages remain in the channel for potential future consumers.
- SPMC (single-producer, multi-consumer) channels are out of scope for this feature but the design should not preclude adding them later.
- This feature builds on top of features 001 (component framework) and 002 (registry, refcounting, binding). Those features are prerequisites.

## Clarifications

### Session 2026-03-31

- Q: What should the default channel queue depth be when the user doesn't explicitly configure one? → A: 1024 elements (power-of-two, balanced for general-purpose use)
- Q: How should actor message-handler panics be reported to the caller/system? → A: Error callback — user provides a handler function at actor creation time
- Q: Should activate/deactivate be idempotent or return an error if already in that state? → A: Return an error (fail-fast on double-activate or double-deactivate)
- Q: When a receiver actor is deactivated, should senders still be able to enqueue messages? → A: Yes — channel accepts messages regardless of receiver state (up to capacity, then blocks)

# Feature Specification: Zyre Rust Bindings

**Feature Branch**: `001-zyre-bindings`  
**Created**: 2026-07-01  
**Status**: Draft  
**Input**: User description: "This component provides Rust bindings for the zyre library found here: https://github.com/zeromq/zyre. Pull the C library into a sub-repo as we've done with SPDK. As much as possible, present a Rust-style API, where that style may differ from a C API style."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Discover and Communicate with Peers on a LAN (Priority: P1)

A Certus node operator creates a Zyre node, starts it, joins a group, and exchanges messages with other Certus nodes on the same network segment. Peer discovery happens automatically via UDP beaconing with no external configuration required.

**Why this priority**: Core functionality — without peer-to-peer messaging over the LAN, the component has no purpose.

**Independent Test**: Can be fully tested by starting two Zyre nodes in separate threads, having them join the same group, exchanging a shout message, and verifying receipt. Delivers the fundamental value of zero-configuration peer discovery and group messaging.

**Acceptance Scenarios**:

1. **Given** two Zyre nodes on the same network, **When** both start and join the same group, **Then** each receives an ENTER event followed by a JOIN event for the other peer within 2 seconds.
2. **Given** two nodes in the same group, **When** one node shouts a message to the group, **Then** the other node receives a SHOUT event containing the message payload.
3. **Given** two nodes in the same group, **When** one node whispers a message to the other by UUID, **Then** the recipient receives a WHISPER event containing the message payload.
4. **Given** a running node, **When** the node is stopped and dropped, **Then** other peers receive an EXIT event within the evasive timeout period.

---

### User Story 2 - Idiomatic Rust API with Ownership and Safety (Priority: P1)

A Rust developer uses the Zyre bindings through an API that follows Rust conventions: RAII resource management, Result-based error handling, iterator-based event streams, and Send+Sync safety where appropriate. The developer should not need to understand the C API or manage raw pointers.

**Why this priority**: Equal priority to core functionality — a non-idiomatic Rust wrapper provides little value over raw FFI. The API must feel native to Rust developers.

**Independent Test**: Can be verified by compiling code that uses the API without any `unsafe` blocks in user code, and by demonstrating that nodes are automatically cleaned up when dropped.

**Acceptance Scenarios**:

1. **Given** a Zyre node created via the Rust API, **When** the node goes out of scope, **Then** the underlying C resources are freed automatically (RAII).
2. **Given** the Rust API, **When** any fallible operation is called, **Then** it returns a `Result<T, ZyreError>` rather than a null pointer or error code.
3. **Given** the Rust API, **When** a developer iterates over incoming events, **Then** they can use a typed enum (`ZyreEvent`) with pattern matching rather than string comparison.
4. **Given** the Rust API, **When** the developer configures node options, **Then** they use a builder pattern rather than calling multiple setter functions after construction.

---

### User Story 3 - Build Integration with Sub-Repo (Priority: P2)

A developer clones the Certus repository and builds the zyre component. The zyre C library (plus its dependencies: libzmq, czmq) is pulled into a `deps/` sub-directory following the same pattern as SPDK. A build script compiles the C dependencies and generates FFI bindings automatically.

**Why this priority**: Without a repeatable build that pulls and compiles zyre from source, the bindings cannot be used on a fresh checkout. Lower than P1 because manual pre-installation is an acceptable short-term workaround.

**Independent Test**: Can be tested by running the build script on a clean checkout and verifying that `cargo build -p zyre` succeeds and produces a working binary.

**Acceptance Scenarios**:

1. **Given** a fresh clone with no pre-installed zyre libraries, **When** the developer runs the dependency build script, **Then** libzmq, czmq, and zyre are cloned, built, and installed into the local `deps/` directory.
2. **Given** the C dependencies are built, **When** `cargo build -p zyre` is run, **Then** the crate compiles successfully, linking against the locally-built libraries.
3. **Given** the sub-repo checkout, **When** the developer inspects the build artifacts, **Then** no system-wide library installation has occurred (all artifacts are under `deps/`).

---

### User Story 4 - Gossip-Based Discovery (Priority: P3)

A Certus node operator configures zyre nodes to use gossip-based discovery instead of UDP beaconing, enabling peer discovery across network segments where broadcast is unavailable (e.g., routed networks, some cloud environments).

**Why this priority**: Gossip is an alternative discovery mechanism for environments without broadcast. Most LAN deployments work with the default UDP beacon, making this a secondary mode.

**Independent Test**: Can be tested by starting nodes with gossip endpoints configured, verifying they discover each other without UDP beaconing active.

**Acceptance Scenarios**:

1. **Given** a node configured with a gossip bind endpoint, **When** another node connects to that gossip endpoint, **Then** both discover each other and can exchange messages.
2. **Given** gossip-configured nodes, **When** a new peer joins, **Then** existing peers receive ENTER events within the gossip propagation interval.

---

### Edge Cases

- What happens when a node attempts to join a group before calling `start()`? The API returns an error.
- What happens when a peer becomes unreachable mid-conversation? The node receives EVASIVE, then SILENT, then EXIT events at configured timeout intervals.
- What happens when two nodes have the same name? They coexist — names are not unique, UUIDs are.
- What happens when the network interface specified for beaconing does not exist? Node start returns an error.
- What happens when a message is sent to a UUID that has already departed? The send completes silently (fire-and-forget semantics, matching zyre's design).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The crate MUST provide safe Rust bindings over the zyre C API with no `unsafe` required in downstream user code.
- **FR-002**: The crate MUST expose node lifecycle management (create, configure, start, stop) via RAII — dropping a node stops it and frees all resources.
- **FR-003**: The crate MUST represent all zyre event types (ENTER, EXIT, EVASIVE, SILENT, JOIN, LEAVE, WHISPER, SHOUT, STOP) as a Rust enum with associated data.
- **FR-004**: The crate MUST support sending messages (whisper and shout) with a single-frame `&[u8]` payload as the primary API, with separate `_multi` variants accepting `&[&[u8]]` for multi-frame messages.
- **FR-005**: The crate MUST provide a builder pattern for node configuration (name, headers, port, interface, timeouts, discovery mode).
- **FR-006**: The crate MUST support both UDP beacon and gossip discovery modes. In gossip mode, the node requires an explicit data endpoint (its ZRE mailbox) set via the builder's `endpoint()` method, distinct from the gossip hub endpoint.
- **FR-007**: The crate MUST expose peer introspection (list peers, list groups, get peer headers/address).
- **FR-008**: The crate MUST provide a direct `recv()` method that blocks the calling thread (thin wrapper over `zyre_recv`) and a non-blocking `try_recv()` for polling. No internal background threads are spawned.
- **FR-009**: The build system MUST clone and compile zyre and its dependencies (libzmq, czmq) from source into `deps/zyre-build/` at the workspace root, mirroring the `deps/spdk-build/` pattern.
- **FR-010**: The crate MUST generate FFI bindings via `bindgen` in a build script, linking against the locally-built libraries.
- **FR-011**: The crate MUST be `Send` (nodes can be moved between threads) but NOT `Sync` (the underlying C API is not thread-safe for concurrent access to a single node).
- **FR-012**: The crate MUST provide typed errors via a `ZyreError` enum covering all failure modes (start failure, invalid configuration, network errors).

### Key Entities

- **ZyreNode**: The primary handle representing a single zyre peer on the network. Owns the underlying `zyre_t` pointer. Created via `ZyreNode::new(config)`.
- **ZyreEvent**: A typed enum representing an incoming network event, carrying the event type, peer UUID, peer name, and optional group/message data.
- **NodeBuilder / NodeConfig**: Configuration for constructing ZyreNode instances with validated parameters.
- **PeerId**: A newtype over UUID string identifying a remote peer.
- **IZyre**: Component interface providing a `ping()` health-check method. Kept minimal to avoid circular dependency between the `interfaces` and `zyre` crates.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Two Zyre nodes discover each other and exchange a round-trip message within 2 seconds on localhost.
- **SC-002**: The public API surface contains zero `unsafe` blocks — all unsafety is encapsulated within the crate's internal FFI layer.
- **SC-003**: Building from a clean checkout (after running the dependency script) completes in under 5 minutes on a standard development machine.
- **SC-004**: All zyre event types (9 types: ENTER, EXIT, EVASIVE, SILENT, JOIN, LEAVE, WHISPER, SHOUT, STOP) are representable in the Rust event enum with no loss of information compared to the C API.
- **SC-005**: Memory safety is maintained — running the test suite under Miri or valgrind produces zero errors related to the bindings.

## Clarifications

### Session 2026-07-01

- Q: Should the zyre C library track master or pin to a specific version? → A: Pin to the latest stable release tag (v2.0.1), matching the SPDK sub-repo precedent for reproducible builds.
- Q: Should the message payload API expose single-frame or multi-frame sends? → A: Single-frame `&[u8]` as the primary API with a separate `_multi` variant for multi-frame use cases.
- Q: Should zyre C dependencies live at the top-level deps/ or within the component? → A: Top-level `deps/zyre-build/` (mirrors `deps/spdk-build/`) for consistency and potential sharing.
- Q: What should the IZyre component interface expose? → A: A `ping()` health-check method. The `ZyreNode::new(config)` factory lives in the `zyre` crate directly — placing it on `IZyre` would create a circular dependency between the `interfaces` and `zyre` crates. Consumers depend on the `zyre` crate for node operations.
- Q: Should event delivery use direct recv or a background thread with channel? → A: Direct `recv()` blocks the calling thread (thin wrapper, no hidden thread). Users spawn their own thread if they want a channel.

## Assumptions

- The target platform is Linux (consistent with the overall Certus project requirements).
- libsodium (CURVE security) is out of scope for the initial version — only plaintext communication is supported.
- The zyre C library is pinned to the latest stable release tag (v2.0.1) for reproducibility, following the SPDK precedent.
- The component integrates into the Certus component framework via `define_component!` and `IZyre` as a health-check interface (`ping()`). The primary API surface is the standalone `ZyreNode` type, created via `ZyreNode::new(config)`.
- Multi-frame ZeroMQ messages are supported via `_multi` method variants; the primary API uses single-frame `&[u8]`.
- Async/await integration (e.g., tokio) is out of scope for v1 — the event API is synchronous with blocking `recv()` and non-blocking `try_recv()`. No internal background threads.

# Research: Zyre Rust Bindings

**Date**: 2026-07-01  
**Feature**: 001-zyre-bindings

## R1: Zyre C Library Build Chain

**Decision**: Build libzmq → czmq → zyre from source using cmake, installed into `deps/zyre-build/` with `CMAKE_INSTALL_PREFIX`.

**Rationale**: Zyre depends on czmq which depends on libzmq. All three use cmake as their primary build system. Building from source into a local prefix avoids system-wide installation and matches the SPDK sub-repo pattern. Pin to release tags: libzmq v4.3.5, czmq v4.2.1, zyre v2.0.1.

**Alternatives considered**:
- pkg-config to find system-installed libraries — rejected: breaks the "no system-wide install" requirement and isn't reproducible across machines.
- vcpkg/conan — rejected: adds an external package manager dependency not used elsewhere in Certus.
- Git submodules — considered but rejected in favor of the clone-in-script approach matching `build_spdk.sh`.

## R2: Bindgen Strategy

**Decision**: Use `bindgen` in `build.rs` to generate Rust FFI bindings from `zyre.h` and `czmq.h` headers at build time. Output to `$OUT_DIR/bindings.rs` and include via `include!()`.

**Rationale**: This is the standard Rust approach for C library bindings (same as `spdk-sys`). Build-time generation ensures bindings always match the compiled library version. Only the headers needed for the public API are processed (not internal headers).

**Alternatives considered**:
- Pre-generated bindings checked into source — rejected: version skew risk, platform-specific differences.
- Manual `extern "C"` declarations — rejected: error-prone for 50+ functions, no type safety guarantees.

## R3: Thread Safety Model

**Decision**: `ZyreNode` implements `Send` but not `Sync`. Each node must be used from a single thread at a time. The `ZyreComponent` (implementing `IZyre`) is `Send + Sync` since the factory method creates independent nodes.

**Rationale**: The zyre C API documentation states that a `zyre_t` instance is not thread-safe. ZeroMQ sockets (which zyre uses internally) must not be shared across threads. However, they can be moved between threads (ownership transfer). The component wrapper itself holds no mutable state and only creates new nodes, so it can safely be `Sync`.

**Alternatives considered**:
- Wrapping in `Mutex<ZyreNode>` for `Sync` — rejected: unnecessary overhead, blocks the design intent of direct recv().
- Actor model with internal thread — rejected per clarification Q5 (no hidden threads).

## R4: Event Parsing Strategy

**Decision**: Parse raw `zmsg_t` messages from `zyre_recv()` into a `ZyreEvent` enum immediately upon receipt. The enum variants carry owned data (Strings, Vec<u8>) so the raw message can be freed.

**Rationale**: The C API returns events as multi-frame ZeroMQ messages where the first frame is a type string. Parsing eagerly into a Rust enum provides type safety and avoids lifetime issues with borrowed C strings. The cost of copying is negligible for typical message sizes (peer names, group names, small payloads).

**Alternatives considered**:
- Lazy parsing with borrowed references into the raw message — rejected: complex lifetime management, message must be kept alive, error-prone.
- Using `zyre_event_t` wrapper from zyre — acceptable but adds another FFI layer; direct `zmsg_t` parsing is simpler and more predictable.

**Decision (revised)**: Use `zyre_event_new()` / `zyre_event_*` accessors from the C API rather than raw zmsg parsing. This is the higher-level C API designed for this purpose.

**Rationale for revision**: The `zyre_event` class handles protocol versioning and internal format changes. Using it ensures forward compatibility with future zyre versions without duplicating parsing logic.

## R5: Builder Pattern Design

**Decision**: Provide a `NodeConfig` struct with a builder API. The builder validates configuration at build time and `IZyre::create_node(config)` consumes it to produce a started or un-started `ZyreNode`.

**Rationale**: Rust builder pattern is idiomatic for complex configuration. Separating config from the live node allows validation before resource allocation. The config can be cloned to create multiple identically-configured nodes.

**Fields**:
- `name: Option<String>` — human-readable name (auto-generated if None)
- `headers: HashMap<String, String>` — metadata shared during discovery
- `port: Option<u16>` — beacon port override (default 5670)
- `interface: Option<String>` — network interface for beaconing
- `evasive_timeout_ms: Option<u32>` — default 5000
- `expired_timeout_ms: Option<u32>` — default 30000
- `beacon_interval_ms: Option<u32>` — default 1000
- `gossip_endpoint: Option<GossipConfig>` — if set, uses gossip instead of beacon

**Alternatives considered**:
- Separate `BeaconConfig` / `GossipConfig` enums — considered; using `GossipConfig` as an option within the main config is simpler.
- Typestate builder (compile-time state machine) — rejected: over-engineering for this use case.

## R6: Error Handling

**Decision**: Single `ZyreError` enum with variants covering distinct failure modes. All FFI functions that can fail are wrapped to return `Result<T, ZyreError>`.

**Variants**:
- `CreateFailed` — zyre_new returned null
- `StartFailed(String)` — zyre_start returned -1, includes context
- `NotStarted` — operation called before start
- `InvalidConfig(String)` — builder validation failure
- `SendFailed` — whisper/shout returned -1
- `RecvFailed` — unexpected null from zyre_recv (node stopped)

**Rationale**: A flat enum is simplest and sufficient. Each variant maps to a specific C API failure mode. The `String` context in some variants carries the detail that the C API doesn't provide (we generate it).

**Alternatives considered**:
- `anyhow::Error` — rejected: library crates should use typed errors.
- Separate error types per module — rejected: over-segmented for 6 variants.

# Research: gRPC Dispatcher Server

## R1: gRPC Framework for Rust

**Decision**: Use `tonic` (gRPC framework) + `prost` (protobuf code generation)

**Rationale**: tonic is the de-facto Rust gRPC framework built on tokio. It provides both server and client code generation from .proto files, TLS support via `rustls` or `native-tls`, and integrates with the tokio async runtime. The project already uses tokio-compatible patterns. prost is the standard protobuf compiler for Rust.

**Alternatives considered**:
- `grpc-rs` (C-core based): heavier dependency, less idiomatic Rust, C build complexity conflicts with existing SPDK build
- Custom TCP protocol: would work but no Python ecosystem tooling; gRPC provides free Python client generation

## R2: Async Runtime Compatibility with SPDK

**Decision**: Use `tokio` runtime for the gRPC server layer; dispatcher calls are blocking (synchronous) and will be dispatched via `tokio::task::spawn_blocking`

**Rationale**: The dispatcher component and its SPDK-backed block devices use synchronous interfaces (no async/await). The gRPC server (tonic) requires tokio. Since requests are serialized (FR-013), each gRPC request handler will call `spawn_blocking` to invoke the dispatcher on a dedicated thread, preventing the tokio event loop from blocking. Serialization is achieved by wrapping the dispatcher reference in a `Mutex` or using a single-threaded work queue.

**Alternatives considered**:
- Multithreaded tokio with concurrent dispatch: rejected because spec requires serialized processing (FR-013)
- Dedicated non-async thread for dispatch + channel: viable but over-engineered for serialized processing

## R3: Python Client Library

**Decision**: Use `grpcio` + `grpcio-tools` for Python client generation from the same .proto file

**Rationale**: Standard Python gRPC stack. The .proto file is shared between server (Rust/tonic) and client (Python/grpcio), ensuring type-safe contract. The existing project uses Python clients (gpu-handle-test-client) so the pattern is established.

**Alternatives considered**:
- `betterproto`: lighter but less mature, fewer features
- REST/HTTP: would require manual serialization; gRPC gives typed stubs for free

## R4: CLI Argument Parsing

**Decision**: Use `clap` with derive macros

**Rationale**: Industry standard for Rust CLI applications. Provides repeatable arguments (`--data-pci` specified multiple times), built-in help/usage generation, and strong type safety. Already used in similar Rust projects.

**Alternatives considered**:
- `structopt`: deprecated in favor of clap derive
- Manual `std::env::args`: too error-prone for complex argument validation

## R5: TLS Support

**Decision**: Use `tonic`'s built-in TLS via `rustls` (enabled via feature flag)

**Rationale**: tonic natively supports TLS with rustls (no OpenSSL dependency). Enable via CLI flags (`--tls-cert`, `--tls-key`). When flags absent, run plaintext. This is a standard tonic pattern.

**Alternatives considered**:
- OpenSSL/native-tls: heavier C dependency, conflicts with SPDK build environment
- mTLS: over-engineered for local-only use case

## R6: Component Wiring Pattern

**Decision**: Follow the `certus-connector/src/engine.rs` pattern for component stack initialization

**Rationale**: The certus-connector already demonstrates the exact wiring sequence: SPDKEnvComponent → GpuServicesComponentV0 → DispatchMapComponentV0 → DispatcherComponentV0, with receptacle binding and interface querying. The gRPC server will use the same stack initialization, just triggered by CLI args instead of Python config dict.

**Alternatives considered**:
- None — this is established project practice

## R7: Serialized Request Processing

**Decision**: Use a `tokio::sync::Mutex` around the dispatcher to serialize gRPC request processing

**Rationale**: FR-013 requires one-at-a-time request processing. A tokio Mutex on the dispatcher Arc ensures only one request handler executes at a time while still allowing the gRPC server to accept connections and queue requests. Since dispatcher operations may block (SPDK I/O), the actual calls happen inside `spawn_blocking` with the mutex held.

**Alternatives considered**:
- `std::sync::Mutex` in spawn_blocking: also viable, simpler; chosen approach depends on implementation ergonomics
- mpsc channel to a single worker thread: cleaner separation but adds complexity for no benefit given single-request semantics

# Zyre Component

Safe, idiomatic Rust bindings for the [zyre](https://github.com/zeromq/zyre) C library (zero-configuration LAN peer discovery and group messaging).

## Build

Requires pre-built C dependencies at `deps/zyre-build/`:

```bash
deps/install_zyre_deps.sh   # System prerequisites
deps/build_zyre.sh          # Build libzmq, czmq, zyre
cargo build -p zyre
cargo test -p zyre
```

## Architecture

- `IZyre` component interface is a factory: `create_node(config) -> Box<dyn IZyreNode>`. It is the only entry point — the concrete `ZyreNode` and its `new()` are crate-private, so callers cannot bypass the interface.
- Node operations (join/leave/shout/whisper/recv/…) live on the `IZyreNode` handle trait.
- `IZyreNode` is a plain `Send` (not `Sync`) trait, deliberately **not** a `define_interface!` component interface: that would force `Send + Sync` + `&self`, requiring a lock around this inherently single-threaded, `&mut self` C resource. As a returned handle it needs no runtime interface discovery.
- Value types (`NodeConfig`, `GossipConfig`, `ZyreEvent`, `PeerId`, `ZyreError`) and the `IZyre`/`IZyreNode` traits live in the `interfaces` crate (so `IZyre` can name them without a crate cycle); the `zyre` crate re-exports them.
- `NodeConfig` is a plain `#[non_exhaustive]` struct with public fields + `Default` (no builder); construct via `let mut c = NodeConfig::default(); c.name = Some(...);`.
- Events delivered via blocking `recv()` / non-blocking `try_recv()` — the bindings add no threads of their own (the zyre C library runs its own discovery/beacon threads internally).
- C dependencies pinned: zyre v2.0.1, czmq v4.2.1, libzmq v4.3.5

For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan at
`specs/001-zyre-bindings/plan.md`

<!-- MANUAL ADDITIONS START -->
<!-- MANUAL ADDITIONS END -->

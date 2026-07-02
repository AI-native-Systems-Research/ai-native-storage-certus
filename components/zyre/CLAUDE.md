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

- `IZyre` component interface acts as a factory: `create_node(config) -> ZyreNode`
- `ZyreNode` is `Send` but not `Sync` (matches C API thread-safety model)
- Events delivered via blocking `recv()` / non-blocking `try_recv()` — no hidden threads
- C dependencies pinned: zyre v2.0.1, czmq v4.2.1, libzmq v4.3.5

For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan at
`specs/001-zyre-bindings/plan.md`

<!-- MANUAL ADDITIONS START -->
<!-- MANUAL ADDITIONS END -->

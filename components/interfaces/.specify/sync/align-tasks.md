# Align Tasks — `interfaces`

**Regenerated**: 2026-09-02 (Spec-Sync)

These are code-side changes required to bring the implementation into line with the
intended contract. Per the sync constraints, `.rs` files are **not** edited by the sync
itself — each task is recorded here for a human/code pass.

## Open

### ALIGN-IFACE-001 — Wire the `iipc` module into `src/lib.rs`

**Severity**: major (latent build break for a real consumer)

`src/iipc.rs` defines `IIpcServer` (+ `IpcServerConfig`, `IpcError`, `IpcMetricsSnapshot`),
but the `iipc` module is never declared or re-exported in `src/lib.rs`, so these symbols
are not part of the compiled/exported `interfaces` crate. The consumer
`components/ipc-component/src/lib.rs:33` does
`use interfaces::{IDispatcher, IIpcServer, ILogger, IpcError, IpcMetricsSnapshot, IpcServerConfig};`
and would fail to compile; the break is masked only because `ipc-component` is not a
workspace member (absent from root `Cargo.toml`) and both files are untracked. This is the
same orphaned-module pattern that FR-014/FR-025 had before it was fixed.

**Fix** (in `src/lib.rs`):
- Add `mod iipc;` alongside the other ungated module declarations.
- Add `pub use iipc::{IIpcServer, IpcError, IpcMetricsSnapshot, IpcServerConfig};` (ungated —
  the interface carries no `spdk`/`gpu` types by design).

**Verification**: `cargo build` and `cargo build --features spdk` still pass; then bring
`components/ipc-component` into the workspace and confirm it compiles against the exported
symbols. (Adding `ipc-component` to the workspace is that component's own concern, not
`interfaces`.)

Spec documentation for this interface is now in place — see **FR-035** (with its
orphaned-module caveat), backfilled in this sync pass.

## Superseded (previously listed, now RESOLVED)

Earlier passes recorded code-side ALIGN tasks against **FR-014** (wire
`mod iextended_metadata_store;` + re-exports into `src/lib.rs`). This is **DONE** and
verified in the current source:

- `src/lib.rs:78` — `mod iextended_metadata_store;` (ungated)
- `src/lib.rs:100` — `pub use iextended_metadata_store::{ExtendedMetadataStoreError, IExtendedMetadataStore};` (ungated)

`IExtendedMetadataStore` and `ExtendedMetadataStoreError` are now part of the compiled,
exported `interfaces` API, so FR-014/FR-025 no longer drift.

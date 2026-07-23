# Spec Drift Report — example-helloworld-dylib

**Generated**: 2026-07-22T22:39:23Z
**Specs analyzed**: 1 (`specs/001-example-helloworld-dylib/spec.md`)

## Summary

| Metric | Count |
|---|---|
| Specs analyzed | 1 |
| Requirements checked (FR + NFR) | 7 |
| Aligned | 6 |
| Drifted | 1 |
| Not implemented | 0 |
| Unspecced features | 0 |

The crate itself (`src/lib.rs`, 24 lines) matches the spec's mechanical requirements exactly: it exports `#[no_mangle] pub fn create_component() -> ComponentRef`, wraps `HelloWorldComponent`, and is declared `crate-type = ["dylib"]`. The one drift is architectural, not mechanical: the spec's and plan's central design rationale — "both the dylib and the host binary dynamically link the same `component-core` and `example-helloworld` shared libraries" (FR-4) — does not match the actual build graph. Verified by a clean rebuild: neither `component-core` nor `example-helloworld` declares `crate-type = ["dylib"]` (no `[lib]` section in either `Cargo.toml`), and `ldd`/`nm` on the freshly built artifacts show both `libexample_helloworld_dylib.so` and the consumer binary `apps/dynamic-loading-example` statically embed their own independent copies of `component_core`/`example_helloworld` symbols; `ldd` lists no shared dependency on either crate as a `.so`. The demo still works because Rust's `TypeId` equality is derived from compile-time type identity (same source + same `rustc`), not from shared runtime linkage — so the *practical* NFR-2 constraint ("same `rustc` version") is what actually keeps `query_interface` working, while the *stated* mechanism (dynamic linking) is not what's happening.

## Per-Spec Findings — `001-example-helloworld-dylib`

### Aligned

| ID | Requirement | Evidence |
|---|---|---|
| FR-1 | Export a `#[no_mangle]` function named `create_component` | `src/lib.rs:20-21` |
| FR-2 | `create_component` returns a `ComponentRef` wrapping a `HelloWorldComponent` | `src/lib.rs:21-24` (`HelloWorldComponent::new()` then `ComponentRef::from(comp as Arc<_>)`) |
| FR-3 | Returned component supports `IGreeter` via `query_interface` | Confirmed end-to-end via `apps/dynamic-loading-example/src/main.rs:75-76` (`query::<dyn IGreeter + Send + Sync>(&*comp)`), which resolves correctly at runtime (built and ran successfully during this analysis) |
| NFR-1 | Must be compiled as `crate-type = ["dylib"]` | `Cargo.toml:9` (`crate-type = ["dylib"]`) |
| NFR-2 | Host and dylib must use the same `rustc` version | Documented as a hard requirement in `src/lib.rs:9-10`; this is in fact the real reason the demo's `TypeId`-based `query_interface` call succeeds (see Drifted item below) |
| NFR-3 | No C-ABI shim or FFI boundary required | `src/lib.rs` contains no `unsafe`, no `#[repr(C)]`, no FFI types — pure Rust-ABI factory function |

### Drifted

| Requirement | Spec Text | Actual | Location | Severity |
|---|---|---|---|---|
| FR-4 / Overview | spec.md:15-17 (Overview) — "Because both the dylib and the host binary dynamically link the same `component-core` and `example-helloworld` shared libraries, `TypeId` values remain consistent across the dylib boundary." spec.md:42 (FR-4) — "TypeId consistency is maintained by dynamically linking shared dependencies." Also plan.md:16-27 (Architecture diagram) shows `component-core.so` and `example-helloworld.so` as shared deps. | Neither `component-core` nor `example-helloworld` declares `crate-type = ["dylib"]` (no `[lib]` section at all in either `Cargo.toml` — default is `rlib`-only). A clean rebuild confirms: `ldd target/debug/libexample_helloworld_dylib.so` shows only `libgcc_s.so.1`, `libc.so.6`, `ld-linux-x86-64.so.2` — no `component_core`/`example_helloworld` shared object. `nm` on that `.so` shows `component_core`/`example_helloworld` symbols compiled directly into it (statically linked as rlibs). `ldd target/debug/dynamic-loading-example` likewise shows no such shared dependency, and `nm` finds `component_core`/`example_helloworld` symbols statically embedded in the host binary too. So the two sides embed **independent, separately compiled copies** of these crates — they are not dynamically linking a shared `.so` as the spec claims. | `components/example-helloworld/Cargo.toml` (no `[lib]`/`crate-type`); `components/component-framework/crates/component-core/Cargo.toml` (no `[lib]`/`crate-type`); observed via `ldd`/`nm` on `target/debug/libexample_helloworld_dylib.so` and `target/debug/dynamic-loading-example` | Medium — functionally harmless (the demo works because `TypeId` equality doesn't actually require shared dynamic linking between same-compiler builds), but the stated architectural rationale is factually wrong and could mislead a maintainer who, e.g., changes one side's `Cargo.lock` resolution or feature set independently, believing a shared `.so` enforces consistency when nothing does |

### Not Implemented

None — all FR/NFR items have corresponding code or behavior.

## Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---|---|---|---|
| — | — | — | None found. `src/lib.rs` contains exactly the one factory function described by FR-1–FR-3; no additional public surface exists beyond what the spec documents. |

## Conflicts

- `plan.md`'s Architecture diagram (lines 16-27) repeats the same "shared deps: `component-core.so`, `example-helloworld.so`" claim as `spec.md`'s Overview/FR-4 — the two design artifacts are internally consistent with each other but both diverge from the actual build graph (see Drifted item above). `tasks.md`'s open checklist item "Confirm `ComponentRef` return type matches the latest `component-core` API" is unresolved but no divergence was found there — `ComponentRef::from(Arc<_>)` matches current `component-core` usage.
- `tasks.md`'s open checklist item "Validate that the dylib is exercised by at least one integration test in the workspace" is unresolved: `apps/dynamic-loading-example/src/main.rs` is a demo `fn main()` binary with no `#[test]` and is not run under `cargo test`. It exercises the dylib manually/visually but is not an automated integration test.

## Recommendations

1. **Fix the drift** — either:
   - (a) Correct `spec.md` (Overview, FR-4) and `plan.md` (Architecture diagram, Key Design Decision #1) to state the actual mechanism: `TypeId` consistency here relies solely on both sides being compiled by the same `rustc` version against the same crate sources/versions (already captured correctly in NFR-2) — there is no dynamic linking of `component-core`/`example-helloworld` as separate `.so` files involved, or
   - (b) If dynamic-linking-based `TypeId` sharing was the original intended design, add `crate-type = ["dylib"]` to `component-core` and `example-helloworld` and pass `-C prefer-dynamic` (or otherwise force Cargo to link against the shared `.so` rather than statically embedding rlibs) so the implementation matches the spec.
   - Option (a) is lower-risk and matches current behavior; option (b) would be a real architecture change purely for documentation accuracy and isn't obviously worth the churn given (a) already works.
2. Convert the `apps/dynamic-loading-example` demo into (or add alongside it) an automated `#[test]`-based integration test, closing the open `tasks.md` item and giving CI coverage of the dylib-loading path (currently only exercised manually via `cargo run -p dynamic-loading-example`).
3. Update `spec.md`'s Status field from "Backfilled" to "Reviewed" once the FR-4/Overview correction above is made, per `tasks.md`'s final checklist item.
4. No action needed on FR-1–FR-3, NFR-1–NFR-3 — all confirmed aligned with the implementation.

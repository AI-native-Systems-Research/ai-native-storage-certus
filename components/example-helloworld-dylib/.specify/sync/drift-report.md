# Drift Report: example-helloworld-dylib

Generated: 2026-08-07T15:29:55Z
Spec: components/example-helloworld-dylib/specs/001-example-helloworld-dylib/spec.md
Implementation: components/example-helloworld-dylib/src/lib.rs, Cargo.toml

## Summary

| Metric | Count |
|--------|-------|
| Aligned | 7 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced | 0 |

The dylib component is functionally aligned with its spec. The only divergence is documentation-level: the source module doc comment describes the cross-boundary `TypeId` mechanism in a way the spec explicitly corrected (claims dynamically-linked shared libraries, whereas the spec states each side statically embeds its own rlib and no `.so` is shared).

## Detailed Findings

### Aligned

- FR-1 `#[no_mangle]` `create_component` — `src/lib.rs:20-21`.
- FR-2 returns `ComponentRef` wrapping `HelloWorldComponent` — `src/lib.rs:22-23`.
- FR-3 returned component supports `IGreeter` via `query_interface` (inherited from example-helloworld) — doc `src/lib.rs:18-19`.
- FR-4 TypeId consistency via same-rustc compilation — realized by depending on the same `component-core`/`example-helloworld` crates (`Cargo.toml [dependencies]`). (Mechanism description drifts — see below.)
- NFR-1 `crate-type = ["dylib"]` — `Cargo.toml [lib]`.
- NFR-2 same-rustc requirement — documented `src/lib.rs:9-10`.
- NFR-3 no C-ABI shim / FFI — pure Rust-ABI factory (`src/lib.rs:20-23`).
- Dependencies match Cargo.toml (`component-core`, `example-helloworld`).

### Drifted

1. **FR-4 mechanism description — source doc contradicts corrected spec** — LOW
   - Spec (Overview + FR-4): each side "statically embeds its own copy" of `component-core`/`example-helloworld` as rlibs; "no shared `.so` linkage is involved"; `TypeId` equality derives from compile-time type identity.
   - Code doc comment: "this dylib and the host binary dynamically link the same `component-core` and `example-helloworld` shared libraries" (`src/lib.rs:4-7`).
   - The runtime behavior is correct/aligned; only the source doc comment carries the outdated (and technically inaccurate) "dynamically link shared libraries" explanation that the spec has since corrected.

## Unspecced Code

None.

## Conflicts / Spec references to nonexistent artifacts

- Success Criteria mention a host application that can `dlopen`/dynamically link the library. No such host/integration artifact is checked into this crate (it is a reference example only); the spec Implementation Notes acknowledge this is a reference pattern. Not a code drift.

## Recommendations

1. Update the module doc comment in `src/lib.rs:4-7` to match the spec's corrected explanation (each side statically embeds its own rlib; `TypeId` equality is compile-time; no shared `.so`). Doc-only change.

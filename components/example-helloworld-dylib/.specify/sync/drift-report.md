# Drift Report: example-helloworld-dylib

**Generated**: pending
**Project**: Certus — components/example-helloworld-dylib

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 7 |
| Aligned | 6 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

## Detailed Findings

### Spec 001-example-helloworld-dylib — example-helloworld-dylib

**Functional Requirements**

- FR-1 ✓ Aligned — `#[no_mangle] pub fn create_component`. `src/lib.rs:20-21`.
- FR-2 ✓ Aligned — returns `ComponentRef` wrapping `HelloWorldComponent`. `src/lib.rs:21-24`.
- FR-3 ✓ Aligned — returned component supports `IGreeter` via `query_interface`; exercised by host app `apps/dynamic-loading-example/src/main.rs:75-76` (`query::<dyn IGreeter…>`).
- FR-4 ⚠️ Drifted (moderate) — **Requirement is satisfied** (TypeId consistency holds via same-source/same-`rustc` compilation), but the **code's own module doc contradicts the corrected spec's mechanism**.
  - Spec text (FR-4 / Implementation Notes): "each side statically embeds its own copy; no shared `.so` linkage is involved."
  - Actual: `src/lib.rs:4-7` states "this dylib and the host binary dynamically link the same `component-core` and `example-helloworld` shared libraries." The same stale claim appears in the host app doc at `apps/dynamic-loading-example/src/main.rs:6-9`.
  - Location: `src/lib.rs:4-7`
  - Severity: moderate — the code documentation asserts an incorrect linkage mechanism that spec.md and plan.md were explicitly corrected to fix.

**Non-Functional Requirements**

- NFR-1 ✓ Aligned — `crate-type = ["dylib"]`. `Cargo.toml:8-9`.
- NFR-2 ✓ Aligned — same-`rustc`-version requirement documented in module doc. `src/lib.rs:9-10`.
- NFR-3 ✓ Aligned — no C-ABI shim / no `extern "C"`; Rust-ABI factory only. `src/lib.rs:20-24`.

**Not Implemented**: none.

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| (none) | — | — | — |

## Recommendations

- Fix the stale linkage description in `src/lib.rs:4-7` (module doc) to match the corrected spec: each side statically embeds its own `rlib` copy of `component-core`/`example-helloworld`; TypeId consistency stems from compile-time type identity, not shared `.so` linkage.
- Apply the same correction to the host-app doc `apps/dynamic-loading-example/src/main.rs:6-9`, which repeats the same inaccurate claim (out of scope for this component's spec, but flagged for consistency).
- Note: plan.md Testing claims an "integration test"; no in-crate `tests/` exists — the behavior is instead exercised by the `apps/dynamic-loading-example` binary. Consider aligning plan.md wording (minor).

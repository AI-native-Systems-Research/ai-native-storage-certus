# Align Tasks — example-helloworld-dylib

Generated from `.specify/sync/drift-report.{json,md}` during spec-sync AUTO-BACKFILL apply (2026-07-22).

## Task 1: Add automated integration test coverage for the dylib-loading path

**Severity**: Low

**Spec Requirement**: `specs/001-example-helloworld-dylib/tasks.md` — "Validate that the dylib is exercised by at least one integration test in the workspace"; `plan.md` Testing section — "Integration test: A host binary loads the dylib, calls `create_component`, queries for `IGreeter`, and invokes a greeting method."

**Current Code**: `apps/dynamic-loading-example/src/main.rs` is a demo `fn main()` binary with no `#[test]` attributes. It exercises the dylib manually (`cargo run -p dynamic-loading-example`) but is not run under `cargo test` and has no automated/CI coverage.

**Required Change**: Add a `#[test]` (or a separate integration test target) that loads `example-helloworld-dylib`, calls `create_component()`, queries the returned `ComponentRef` for `IGreeter`, and asserts on the greeting output — so the dylib-loading path is exercised automatically under `cargo test --all`.

**Files to Modify**: `apps/dynamic-loading-example/src/main.rs` (or a new `apps/dynamic-loading-example/tests/*.rs`); `components/example-helloworld-dylib/src/lib.rs` unaffected.

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`)

Regenerated drift report (2026-08-07T15:29:55Z): 7 aligned, 1 drifted (Low,
doc-only), 0 not-implemented, 0 unspecced. The single drift is an ALIGN item
(source doc comment contradicts the corrected spec). Per sweep pacing, non-HIGH
ALIGN items are **queued, not drafted** (only HIGH code bugs get a drafted
fix). Queued below as Task 2. No spec BACKFILL was needed — the spec is correct;
the code doc comment is stale.

## Task 2: Align FR-4 — correct the stale module doc comment (Low, doc-only)

**Severity**: Low (documentation-only; runtime behaviour is already correct/aligned).

**Spec Requirement**: FR-4 + Overview — each side "statically embeds its own
copy" of `component-core`/`example-helloworld` as rlibs; "no shared `.so`
linkage is involved"; `TypeId` equality derives from compile-time type identity
(same-`rustc` compilation).

**Current Code**: the module doc comment (`src/lib.rs:4-7`) states that "this
dylib and the host binary **dynamically link the same** `component-core` and
`example-helloworld` **shared libraries**". This is the outdated (and
technically inaccurate) explanation the spec has since corrected — each side
statically embeds its own rlib; there is no shared `.so`. Only the comment is
wrong; the factory (`create_component`) and the `crate-type = ["dylib"]` build
are already spec-aligned (NFR-1/NFR-2/NFR-3).

**Required Change**: rewrite `src/lib.rs:4-7` to describe the mechanism the spec
now specifies: each side statically embeds its own copy of the shared crates as
rlibs; `TypeId` equality is a compile-time property guaranteed by building both
sides with the same `rustc`; no `.so` is shared across the boundary. Doc-only;
no logic change.

**Files to Modify**: `components/example-helloworld-dylib/src/lib.rs` (lines 4-7).

**Estimated Effort**: Trivial (doc-only).

### Acceptance Criteria
- [ ] `src/lib.rs:4-7` no longer claims the dylib and host "dynamically link the same shared libraries".
- [ ] The comment states each side statically embeds its own rlib and that `TypeId` equality is a same-`rustc` compile-time property (matching FR-4 / Overview).
- [ ] No functional/logic change; `cargo test -p example-helloworld-dylib` still passes.

---

# 2026-08-20 Phase B (per `.specify/sync/PHASE_B_POLICY.md`)

Regenerated drift report (2026-08-20T09:24): 7 checked, 6 aligned, **1 drifted**
(FR-4, moderate), 0 not-implemented, 0 unspecced. Classified FR-4 as **ALIGN**:
the spec (FR-4 / Overview) is already correct, and the runtime behaviour
satisfies it; the drift is the source module doc comment (`src/lib.rs:4-7`) still
carrying the stale "dynamically link the same shared libraries" claim that
contradicts the corrected spec (spec→code direction, not spec-lag ⇒ not
BACKFILL). Per policy the `.rs` source is not edited. This ALIGN item is
**Task 2** above (still open) — no new task is created, as Task 2 already covers
FR-4 exactly. No spec BACKFILL was applied (spec text already correct).

# Align Tasks — example-helloworld-dylib

Generated from `.specify/sync/drift-report.{json,md}` during spec-sync AUTO-BACKFILL apply (2026-07-22).

## Task 1: Add automated integration test coverage for the dylib-loading path

**Severity**: Low

**Spec Requirement**: `specs/001-example-helloworld-dylib/tasks.md` — "Validate that the dylib is exercised by at least one integration test in the workspace"; `plan.md` Testing section — "Integration test: A host binary loads the dylib, calls `create_component`, queries for `IGreeter`, and invokes a greeting method."

**Current Code**: `apps/dynamic-loading-example/src/main.rs` is a demo `fn main()` binary with no `#[test]` attributes. It exercises the dylib manually (`cargo run -p dynamic-loading-example`) but is not run under `cargo test` and has no automated/CI coverage.

**Required Change**: Add a `#[test]` (or a separate integration test target) that loads `example-helloworld-dylib`, calls `create_component()`, queries the returned `ComponentRef` for `IGreeter`, and asserts on the greeting output — so the dylib-loading path is exercised automatically under `cargo test --all`.

**Files to Modify**: `apps/dynamic-loading-example/src/main.rs` (or a new `apps/dynamic-loading-example/tests/*.rs`); `components/example-helloworld-dylib/src/lib.rs` unaffected.

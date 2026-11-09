# Drift Report: example-helloworld

**Generated**: pending
**Project**: Certus — components/example-helloworld

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 10 |
| Aligned | 10 |
| Drifted | 0 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

Status: **CLEAN** — implementation fully aligned with the backfilled spec.

## Detailed Findings

### Spec 001-example-helloworld — Example Hello World Component

**Functional Requirements**

- FR-1 ✓ Aligned — `IGreeter` interface with `greeting_prefix(&self) -> &str`. `src/lib.rs:25-29`.
- FR-2 ✓ Aligned — `HelloWorldComponent` provides `IGreeter`, returns `"Hello"`. `src/lib.rs:32-46`.
- FR-3 ✓ Aligned — `ILogger` receptacle declared in `define_component!`. `src/lib.rs:36-38`.
- FR-4 ✓ Aligned — `GreetRequest` carries `name: String`. `src/lib.rs:49-52`.
- FR-5 ✓ Aligned — `GreeterHandler` implements `ActorHandler<GreetRequest>` with `on_start`/`handle`/`on_stop`. `src/lib.rs:84-109`.
- FR-6 ✓ Aligned — `handle()` increments `count` and prints numbered greeting. `src/lib.rs:92-98`.
- FR-7 ✓ Aligned — optional `ILogger` logs on start, per greeting, and stop. `src/lib.rs:85-108`.

**Non-Functional Requirements**

- NFR-1 ✓ Aligned — module-level doc comment with runnable `Quick start` example. `src/lib.rs:1-17`.
- NFR-2 ✓ Aligned — `Default` delegates to `new()` (satisfies `clippy::new_without_default`); no obvious lint issues. `src/lib.rs:78-82`.
- NFR-3 ✓ Aligned — no `unsafe` code present (grep clean).

**Drifted**: none.

**Not Implemented**: none.

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| (none) | — | — | — |

## Recommendations

- No action required. Implementation matches spec; dependency table (`component-framework`, `component-core`, `interfaces`, `logger`) matches `Cargo.toml:8-12`.
- Optional (future, per plan.md): add an explicit unit-test module with assertions; coverage currently relies on the doc test only.

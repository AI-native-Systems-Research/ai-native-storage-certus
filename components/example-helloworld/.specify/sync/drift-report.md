# Spec Drift Report — example-helloworld

**Generated**: 2026-07-22T21:31:26Z
**Specs analyzed**: 1 (`specs/001-example-helloworld/spec.md`)

## Summary

| Metric | Count |
|---|---|
| Specs analyzed | 1 |
| Requirements checked (FR + NFR) | 10 |
| Aligned | 9 |
| Drifted | 1 |
| Not implemented | 0 |
| Unspecced features | 0 |

The component's `src/lib.rs` implementation matches its backfilled spec almost exactly (verified `cargo test -p example-helloworld --doc` passes and `cargo clippy -p example-helloworld -- -D warnings` is clean). The one drift is not in a requirement's core behavior but in a **narrative claim about the integration example**: the spec and plan both assert that `apps/helloworld-mainline/` demonstrates logger wiring, but that app never wires a logger.

## Per-Spec Findings — `001-example-helloworld`

### Aligned

| ID | Requirement | Evidence |
|---|---|---|
| FR-1 | `IGreeter` interface with `greeting_prefix(&self) -> &str` | `src/lib.rs:25-29` |
| FR-2 | `HelloWorldComponent` provides `IGreeter`, returns `"Hello"` | `src/lib.rs:32-46` |
| FR-3 | `HelloWorldComponent` declares `ILogger` receptacle | `src/lib.rs:36-38` (`receptacles: { logger: ILogger }`) |
| FR-4 | `GreetRequest` carries `name: String` | `src/lib.rs:49-52` |
| FR-5 | `GreeterHandler` implements `ActorHandler<GreetRequest>` with lifecycle hooks | `src/lib.rs:84-109` (`on_start`, `handle`, `on_stop`) |
| FR-6 | `handle()` increments counter, prints numbered greeting | `src/lib.rs:92-98` (`self.count += 1; ... println!("  [{}] Hello, {}!", ...)`) |
| FR-7 | Optional `ILogger` logs on start/greet/stop | `src/lib.rs:85-108` (`if let Some(log) = &self.logger { log.info(...) }` in all three hooks) |
| NFR-1 | Module doc comment with runnable Quick start example | `src/lib.rs:1-17`; confirmed passing via `cargo test -p example-helloworld --doc` (1 passed) |
| NFR-2 | Zero clippy warnings under `-D warnings` | Confirmed via `cargo clippy -p example-helloworld -- -D warnings` → clean |
| NFR-3 | No unsafe code | No `unsafe` blocks in `src/lib.rs` |

### Drifted

| Requirement | Spec Text | Actual | Location | Severity |
|---|---|---|---|---|
| Implementation Notes claim (spec.md) / Testing claim (plan.md) | spec.md:99 — "A full integration example wiring this component with a logger lives in `apps/helloworld-mainline/`." plan.md:63 — "Integration: The `apps/helloworld-mainline/` application provides a full integration test with logger wiring." | `apps/helloworld-mainline/src/main.rs` instantiates `GreeterHandler::new()` (no logger) and the app's `Cargo.toml` does not even depend on the `logger` crate. There is no logger wiring anywhere in the app. | `apps/helloworld-mainline/src/main.rs:26` (`Actor::simple(GreeterHandler::new())`); `apps/helloworld-mainline/Cargo.toml:8-10` (deps: only `component-framework`, `example-helloworld`) | Medium — misleads a new developer (the component's stated onboarding audience) into believing they can see logger wiring in the reference app when they cannot; User Story 2's acceptance criteria are only exercised by unit-level code inspection, not by the referenced example |

### Not Implemented

None — all FR/NFR items have corresponding code.

## Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---|---|---|---|
| — | — | — | None found. `src/lib.rs` contains exactly the interface, component, message, and handler described by FR-1–FR-7; no additional public surface (e.g. no extra methods, no `IUnknown::query_interface` demonstration in the component itself) exists beyond what the spec documents. |

Note: `apps/helloworld-mainline/src/main.rs` does use `component_framework::iunknown::query::<dyn IGreeter + Send + Sync>()` to demonstrate runtime interface discovery — this lives in the *app*, not in `example-helloworld` itself, so it is out of scope for this component's spec but is worth noting since plan.md's "Future Considerations" (line 70) lists "Could showcase `IUnknown::query_interface()` usage" as an open idea, when a form of it is arguably already demonstrated one layer up, in the app.

## Conflicts

None found between spec.md, plan.md, and tasks.md — all three consistently describe the same (slightly inaccurate) picture of logger wiring in the mainline app.

## Recommendations

1. **Fix the drift** (spec text vs. reality) — either:
   - (a) Update `apps/helloworld-mainline/src/main.rs` to actually call `GreeterHandler::with_logger(...)` with a wired `logger` component, matching what the spec and plan describe, or
   - (b) Correct `spec.md` (Implementation Notes) and `plan.md` (Testing section) to state that logger wiring is demonstrated only via unit-level construction (`GreeterHandler::with_logger`) and is not currently exercised in `apps/helloworld-mainline/`.
   - Option (a) is preferable since it better serves the stated purpose of the app ("a full application that wires up and drives this component" per README.md:15) and closes the gap between User Story 2's intent and observable behavior.
2. Resolve the open item in `tasks.md` ("Decide whether `IGreeter` interface should be promoted to the shared `interfaces` crate") — currently unresolved and not reflected as a decision anywhere.
3. Consider adding an explicit unit test module (`#[cfg(test)]`) per `tasks.md`'s "Optional Improvements" — presently the only test coverage is the single doc test, so `GreeterHandler::with_logger` behavior (User Story 2) has no automated assertion beyond visual inspection.
4. No urgent action required otherwise — the component's core implementation is fully aligned with its spec and builds/lints cleanly.

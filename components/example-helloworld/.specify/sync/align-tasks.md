# Align Tasks — example-helloworld

Generated from `drift-report.json`/`drift-report.md` (2026-07-22) during AUTO-BACKFILL apply.
These are code-side follow-ups that spec text edits alone cannot resolve (per HARD RULES,
spec-sync must not touch source code).

## Task 1 — Mainline app does not actually demonstrate logger wiring

**Severity**: Medium

**Spec Requirement**: `spec.md` (Implementation Notes, pre-fix) and `plan.md` (Testing
section, pre-fix) both asserted that `apps/helloworld-mainline/` provides "a full
integration example wiring this component with a logger." This was the documented intent
behind User Story 2 ("Demonstrating Receptacle Wiring").

**Current Code**: `apps/helloworld-mainline/src/main.rs:26` constructs the actor via
`Actor::simple(GreeterHandler::new())` — no logger is passed to `GreeterHandler::with_logger(...)`.
`apps/helloworld-mainline/Cargo.toml:8-10` lists only `component-framework` and
`example-helloworld` as dependencies; it does not depend on the `logger` crate at all.
As a result, `ILogger` wiring (User Story 2 in `example-helloworld`'s spec) is exercised
only via unit/doc-test-level construction inside `example-helloworld` itself, never by the
reference application a new developer is pointed at.

**Required Change**: Update `apps/helloworld-mainline/src/main.rs` to construct the
`GreeterHandler` via `GreeterHandler::with_logger(...)`, wiring in a concrete `logger`
component instance, and add the `logger` crate as a dependency of
`apps/helloworld-mainline/Cargo.toml`. This is the drift report's preferred resolution
(Option (a)) since it makes the reference app actually demonstrate what the component's
spec/README describe. (As an interim measure, spec-sync has already corrected the spec
text itself — see `specs/001-example-helloworld/spec.md` Implementation Notes and
`plan.md` Testing section — to accurately describe current, logger-less behavior in the
app; this task tracks the preferred follow-up of fixing the app to match original intent.)

**Files to Modify**:
- `apps/helloworld-mainline/src/main.rs`
- `apps/helloworld-mainline/Cargo.toml`

**Status**: Deferred (source-code change; out of scope for spec-sync apply)

---

## Task 2 — Open design decision: promote `IGreeter` to shared `interfaces` crate

**Severity**: Low

**Spec Requirement**: N/A — this is an open item already tracked in
`specs/001-example-helloworld/tasks.md` ("Decide whether `IGreeter` interface should be
promoted to the shared `interfaces` crate"), not a spec/code drift.

**Current Code**: `IGreeter` is defined locally in `components/example-helloworld/src/lib.rs`
via `define_interface!`, as `spec.md`'s Implementation Notes already correctly describe.

**Required Change**: None required by spec-sync. This is a design decision for component
owners, not a drift finding. Left as-is in `tasks.md`; noted here only for visibility.

**Files to Modify**: None (informational only)

**Status**: Ambiguous / no action — deferred to component owner discretion

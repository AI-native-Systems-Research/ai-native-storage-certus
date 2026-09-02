# Align Tasks

Deferred code-side follow-ups identified during spec-sync (AUTO-BACKFILL apply, 2026-07-22).
These are NOT applied automatically — they require a human decision and a code change.

---

## Task: Align 003-actor-channels/FR-004, 005-numa-aware-actors/FR-001

**Severity**: Minor (defense-in-depth / API consistency, not a functional bug)

**Spec Requirement**:
- 003-actor-channels FR-004: "Calling activate on an already-active actor MUST return an error. Double-deactivation MUST be prevented — either by returning a runtime error or by using type-level enforcement..."
- 005-numa-aware-actors FR-001 (already reflects intent): "Actors are single-use; re-activation requires constructing a new Actor instance."

Both FRs establish the pattern that actor lifecycle misuse is reported via `Result::Err` (or prevented at the type level), never via a panic.

**Current Code**:
`Actor::activate()` in `crates/component-core/src/actor.rs:604-617` correctly returns `ActorError::AlreadyActive` when the actor is currently running (CAS on `state`). However, if `activate()` is called a *second* time after a full activate/deactivate cycle (state has returned to `Idle`, so the CAS succeeds), the method panics via:
```rust
.expect("receiver already taken — actor activated twice without reset")
.expect("handler already taken — actor activated twice without reset")
```
because `handler`/`receiver` were already consumed by `Option::take()` on the first activation. This is a misuse path that crashes the host thread instead of returning a typed error, which is inconsistent with FR-004's own "runtime error, not panic" pattern and with the rest of the `ActorError` API surface.

**Required Change**: Replace the two `.expect(...)` calls with a typed error return, e.g. a new `ActorError::AlreadyConsumed` (or reuse `AlreadyActive` with updated docs) returned from `activate()` before spawning the thread, so that calling `activate()` twice on the same `Actor` instance (even across a deactivate cycle) fails gracefully with `Result::Err` instead of panicking. No spec change is required — both FR-004 (003) and FR-001 (005) already describe the intended single-use semantics and error-not-panic pattern; only the implementation needs to catch up.

**Files to Modify**:
- `components/component-framework/crates/component-core/src/actor.rs` (lines ~604-617, `Actor::activate()`; possibly the `ActorError` enum definition near line 24)

---

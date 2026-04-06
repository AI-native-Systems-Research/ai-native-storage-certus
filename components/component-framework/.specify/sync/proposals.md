# Drift Resolution Proposals

Generated: 2026-04-06T12:30:00Z
Based on: drift-report from 2026-04-06T12:00:00Z

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code to Spec) | 5 |
| Align (Spec to Code) | 7 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 1 |

## Proposals

### Proposal 1: 005/FR-004 — Wire validate_cpus() into Actor::activate()

**Direction**: ALIGN (Spec to Code)

**Current State**:

- Spec says: "The framework MUST validate CPU IDs against the system's available CPUs and return an error for invalid IDs before spawning the thread"
- Code does: `Actor::activate()` calls `set_thread_affinity()` directly without calling `validate_cpus()` first. The `validate_cpus()` function exists at `cpuset.rs:355` but is never invoked from the actor path. Invalid CPUs produce generic kernel errors instead of structured `NumaError::CpuOffline(id)` errors.

**Proposed Resolution**:

Add a `validate_cpus()` call in `Actor::activate()` before spawning the thread. Insert validation between the affinity clone and the `thread::spawn` call at `actor.rs:607-612`:

```rust
// After: let affinity = self.cpu_affinity.lock().unwrap().clone();
// Before: let (startup_tx, startup_rx) = ...
if let Some(ref cpus) = affinity {
    crate::numa::validate_cpus(cpus).map_err(|e| ActorError::AffinityFailed(e.to_string()))?;
}
```

This validates CPUs on the calling thread (fast sysfs read) before incurring thread spawn overhead. The validation returns `NumaError::CpuOffline(cpu_id)` for offline CPUs, which is more informative than a generic `EINVAL` from `sched_setaffinity`.

**Rationale**: The spec is authoritative. The validation function was written specifically for this purpose but never wired in — this is a straightforward omission, not an intentional design choice.

**Confidence**: HIGH

**Action**:

- File: `crates/component-core/src/actor.rs:607-612`
- Add `validate_cpus()` call before thread spawn
- Add test: activate actor with invalid CPU ID, assert `ActorError::AffinityFailed` containing "offline"

---

### Proposal 2: 005/FR-016 — NUMA-local channel buffer allocation

**Direction**: BACKFILL (Code to Spec)

**Current State**:

- Spec says: "Channel buffers SHOULD be allocated on the NUMA node associated with the actor's CPU affinity"
- Code does: `SpscChannel::new_numa(capacity, _node)` and `MpscChannel::new_numa(capacity, _node)` accept a node parameter but discard it, delegating to `Self::new(capacity)`. The spec itself already documents this: "relies on first-touch rather than explicit `mbind()` for buffer placement, as explicit memory policy changes interfere with the Rust allocator's internal bookkeeping."

**Proposed Resolution**:

Update spec 005-FR-016 to accurately reflect the intentional first-touch-only design. The spec text already contains this clarification in its own body, but the requirement wording uses SHOULD which creates ambiguity. Replace with:

```
- **FR-016**: Channel buffers rely on Linux first-touch memory policy for
  NUMA locality. The `new_numa()` constructors accept a node parameter for
  API consistency and documentation but delegate to the standard constructor.
  When the constructing thread is pinned to CPUs on a specific NUMA node,
  the OS allocates channel buffer pages on that node when first accessed.
  Explicit `mbind()` is not used for channel buffers because it interferes
  with the Rust allocator's internal bookkeeping.
```

**Rationale**: The code's approach is intentional and documented. The spec's own clarification section already explains the first-touch rationale. The `NumaAllocator` (which does use `mmap`+`mbind`) exists for cases where users need explicit placement, but integrating it into the ring buffer internals would require replacing Rust's `Vec`/`Box` allocations with raw `mmap` — a significant complexity increase with marginal benefit given first-touch works correctly when threads are pinned. The SHOULD keyword already indicates this is advisory.

**Confidence**: HIGH

**Action**:

- File: `specs/005-numa-aware-actors/spec.md` — update FR-016 text

---

### Proposal 3: 005/FR-017 — NUMA-local handler state allocation

**Direction**: REMOVE FROM SPEC

**Current State**:

- Spec says: "Actor handler state SHOULD be allocated on the NUMA node associated with the actor's CPU affinity where possible"
- Code does: No framework integration point exists. The spec itself already acknowledges this: "Explicit NUMA-local allocation of handler state is not provided; users requiring strict placement should construct the handler on a pinned thread."

**Proposed Resolution**:

Reword FR-017 to document the first-touch approach as the implementation, removing the implication that framework integration is needed:

```
- **FR-017**: Actor handler state achieves NUMA-local placement through
  first-touch policy when the handler is constructed on a thread pinned to
  the target NUMA node's CPUs. The framework does not provide explicit
  NUMA-local allocation for handler state; users requiring strict placement
  should construct the handler on a pinned thread before passing it to the
  actor constructor.
```

**Rationale**: The spec already acknowledges this is the intended design. Adding a framework-level allocator integration (e.g., `with_numa_allocator()` builder) would add significant API complexity for a niche use case that first-touch already handles. The `NumaAllocator` is available for users who need explicit control.

**Confidence**: HIGH

**Action**:

- File: `specs/005-numa-aware-actors/spec.md` — update FR-017 text

---

### Proposal 4: 003/FR-002 — Actor bypasses define_component! macro

**Direction**: BACKFILL (Code to Spec)

**Current State**:

- Spec says: "Actor components MUST use the same component, interface, and receptacle model as plain components (defined via the existing macro system)"
- Code does: `Actor<M,H>` is a hand-coded generic struct with manual `IUnknown` impl at `actor.rs:689-731`. It cannot use `define_component!` because it is generic over `M` and `H`, and the macro system does not support generic type parameters. The actor correctly implements the same conceptual model (IUnknown, provided interfaces, receptacles, introspection).

**Proposed Resolution**:

Update spec 003-FR-002 to clarify that actors must conform to the component model (IUnknown trait, interfaces, receptacles, introspection) but may implement it directly rather than through the macro:

```
- **FR-002**: Actor components MUST conform to the same component, interface,
  and receptacle model as plain components — implementing `IUnknown` with
  query, version, provided_interfaces, and receptacles methods. Actors MAY
  implement `IUnknown` directly rather than through `define_component!` when
  generics or other language features require it. The observable behavior
  (introspection, interface query, third-party binding compatibility) MUST
  be identical to macro-generated components.
```

**Rationale**: The code approach is correct. `define_component!` is a `macro_rules!`/proc-macro that generates a concrete struct — it fundamentally cannot produce `Actor<M, H>` where `M` and `H` are type parameters. Forcing actors through the macro would require either: (a) making the macro support generics (significant complexity), or (b) removing generics from Actor (losing type safety). Neither is justified when the hand-coded impl is correct and tested.

**Confidence**: HIGH

**Action**:

- File: `specs/003-actor-channels/spec.md` — update FR-002 text

---

### Proposal 5: 003/FR-019 — Third-party binding for actors/channels

**Direction**: ALIGN (Spec to Code)

**Current State**:

- Spec says: "Actor and channel components MUST support both first-party and third-party binding"
- Code does: Actors implement `IUnknown` with `provided_interfaces()` returning `ISender<M>` info, but no test or example demonstrates wiring an actor via the string-name `bind()` function. All actor-channel wiring uses first-party binding (direct `sender()`/`pipe()` calls).

**Proposed Resolution**:

Add an integration test demonstrating third-party binding for actor-to-channel wiring. The actor already has the correct `IUnknown` metadata (`provided_interfaces` returns `ISender`), so `bind()` should work. However, actors expose `ISender` as a *provided* interface (not a receptacle), and channels also expose `ISender`/`IReceiver` as provided interfaces with no receptacles. This means `bind()` cannot wire them directly — it connects a provider's interface to a consumer's receptacle, but neither actors nor channels have receptacles.

The actual wiring pattern for actors is: query `ISender`/`IReceiver` from actors/channels via `IUnknown`, then use them directly. This is fundamentally different from the receptacle-based `bind()` pattern.

Add a test that demonstrates the IUnknown-based wiring path:

```rust
#[test]
fn actor_channel_wiring_via_iunknown() {
    // Create channel and actor
    let channel = SpscChannel::<String>::new(16);
    let actor = Actor::simple(handler);

    // Query ISender from actor via IUnknown (third-party discovery)
    let sender: Arc<dyn ISender<String>> = query::<dyn ISender<String>>(&actor).unwrap();

    // Query IReceiver from channel via IUnknown (third-party discovery)
    let receiver: Arc<dyn IReceiver<String>> = query::<dyn IReceiver<String>>(&channel).unwrap();

    // Use them to communicate
    sender.send("hello".into()).unwrap();
    // ...
}
```

**Rationale**: The spec is partially correct — actors/channels should be wirable via third-party mechanisms. But the mechanism is IUnknown interface query, not receptacle-based `bind()`, because actors/channels are infrastructure components that expose endpoints rather than consume them. The test should demonstrate the IUnknown-based discovery path.

**Confidence**: MEDIUM

**Action**:

- File: `crates/component-framework/tests/actor.rs` — add IUnknown-based wiring test
- File: `specs/003-actor-channels/spec.md` — clarify FR-019 to distinguish IUnknown query-based wiring from receptacle-based binding

---

### Proposal 6: 003/FR-018 — Channel components in registry

**Direction**: ALIGN (Spec to Code)

**Current State**:

- Spec says: "Actor and channel components MUST be registerable in the existing component registry"
- Code does: Only actors are tested with the registry. Channels implement `IUnknown` but have never been registered as named factories.

**Proposed Resolution**:

Add an integration test in `tests/actor.rs` (or a new `tests/channel_registry.rs`) that registers channel factories and creates channels by name:

```rust
#[test]
fn channel_registerable_in_registry() {
    let registry = ComponentRegistry::new();
    registry.register_simple("spsc-u64", || {
        ComponentRef::from(Arc::new(SpscChannel::<u64>::new(1024)) as Arc<dyn IUnknown>)
    }).unwrap();

    let comp = registry.create("spsc-u64", None).unwrap();
    assert!(comp.provided_interfaces().iter().any(|i| i.name == "ISender"));
}
```

**Rationale**: The spec is authoritative. Channels already implement `IUnknown`, so registration should work. This is a testing gap, not a code gap.

**Confidence**: HIGH

**Action**:

- File: `crates/component-framework/tests/actor.rs` or new test file — add channel registry test

---

### Proposal 7: 002/FR-018 — Initial reference count semantics

**Direction**: BACKFILL (Code to Spec)

**Current State**:

- Spec says: "Factory functions MUST return components with an initial reference count of 1"
- Code does: Macro-generated components store `Arc` clones of themselves in the interface map (so `IUnknown::query_interface_raw` can return references). This means `Arc::strong_count` is > 1 for any macro-generated component. Tests use a `base_count` pattern to work around this. The behavior is correct — the caller holds exactly one `ComponentRef`, and the component is destroyed when all external references are dropped.

**Proposed Resolution**:

Update spec 002-FR-018 to describe the observable semantic rather than the implementation detail:

```
- **FR-018**: Factory functions MUST return components wrapped in a single
  `ComponentRef`. The caller holds exactly one external reference. Internal
  reference count may be higher due to implementation details (e.g., the
  interface map holds Arc clones for query support). The component MUST be
  destroyed when all external `ComponentRef` handles and receptacle
  connections are dropped.
```

**Rationale**: The current implementation is correct — components are destroyed deterministically when all external references drop. The "ref count of 1" invariant is misleading because `Arc::strong_count` includes internal self-references that are an implementation detail of `define_component!`. Changing the macro to avoid self-references would require a fundamentally different interface query mechanism (e.g., raw pointer casting), which would be less safe.

**Confidence**: HIGH

**Action**:

- File: `specs/002-registry-refcount-binding/spec.md` — update FR-018 text

---

### Proposal 8: 002/FR-020 — register_simple integration test

**Direction**: ALIGN (Spec to Code)

**Current State**:

- Spec says: "The registry MUST provide a simplified factory registration method (register_simple)"
- Code does: `register_simple` is implemented at `registry.rs:147-152` with a doc-test, but no integration test in `tests/registry.rs` exercises it.

**Proposed Resolution**:

Add an integration test to `tests/registry.rs`:

```rust
#[test]
fn register_simple_creates_component() {
    let registry = ComponentRegistry::new();
    registry
        .register_simple("simple-counter", || {
            ComponentRef::from(CounterComponent::new(99))
        })
        .unwrap();

    let comp = registry.create("simple-counter", None).unwrap();
    let counter: Arc<dyn ICounter + Send + Sync> =
        query::<dyn ICounter + Send + Sync>(&*comp).unwrap();
    assert_eq!(counter.count(), 99);
}
```

**Rationale**: The implementation is correct; this is purely a test coverage gap.

**Confidence**: HIGH

**Action**:

- File: `crates/component-framework/tests/registry.rs` — add test

---

### Proposal 9: 001/FR-007 + SC-004 — Compile-fail tests for macros

**Direction**: ALIGN (Spec to Code)

**Decision**: Use `compile_fail` doc tests (no extra dependencies).

**Current State**:

- Spec says: "The framework MUST produce compile-time errors when macro usage is incorrect"
- Code does: The macros produce compile-time errors but no automated test validates that incorrect usage is rejected.

**Proposed Resolution**:

Add `compile_fail` doc tests to the macro documentation covering key error paths:

````rust
/// ```compile_fail
/// use component_framework::define_interface;
/// define_interface! { pub IEmpty { } }  // no methods — must fail
/// ```
///
/// ```compile_fail
/// use component_framework::define_interface;
/// define_interface! { pub IBad { fn mutate(&mut self); } }  // &mut self — must fail
/// ```
````

No extra dependencies needed. Tests live alongside the code they validate.

**Confidence**: HIGH

---

### Proposal 10: 001/FR-012 — Prelude completeness

**Direction**: ALIGN (Spec to Code)

**Current State**:

- Spec says: "The framework MUST provide a prelude module that re-exports the most commonly used types"
- Code does: `prelude.rs` re-exports most types but is missing `Receptacle` and `query_interface!` macro.

**Proposed Resolution**:

Add `Receptacle` to the prelude re-exports:

```rust
pub use crate::receptacle::Receptacle;
```

The `query_interface!` macro is already accessible after `use component_core::prelude::*` because `#[macro_export]` places it at the crate root. However, it should be documented in the prelude module docs as available. Add a comment:

```rust
// Note: `query_interface!` is available via `#[macro_export]` at the crate root
// and does not need an explicit re-export here.
```

**Rationale**: `Receptacle` is commonly used by anyone defining components with required interfaces. Its absence from the prelude is an oversight.

**Confidence**: HIGH

**Action**:

- File: `crates/component-core/src/prelude.rs` — add `Receptacle` re-export and doc note

---

### Proposal 11: 001/SC-005 — Benchmark regression gate in CI

**Direction**: ALIGN (Spec to Code) + BACKFILL (Spec update)

**Decision**: Compile-only verification with manual regression checking.

**Current State**:

- Spec says: "All Criterion benchmarks demonstrate no regressions between releases"
- Code does: Benchmarks exist and run with `cargo bench`, but CI only runs `fmt`, `clippy`, `test`, `doc`.

**Proposed Resolution**:

1. Update the CI gate in `CLAUDE.md` to include `cargo bench --no-run` (ensures benchmarks compile).
2. Update SC-005 to: "All Criterion benchmarks MUST compile without errors. Regression detection is performed manually before releases using `cargo bench`."

**Confidence**: HIGH

---

### Proposal 12: 001/SC-004 — Doc test gaps

**Direction**: ALIGN (Spec to Code)

**Current State**:

- Spec says: "100% of public APIs have passing doc tests"
- Code does: Most APIs have doc tests. Gaps: `InterfaceMap::info()` lacks a doc test; the `prelude` module doc test compiles but asserts nothing.

**Proposed Resolution**:

Add meaningful doc tests to the identified gaps. Make the prelude doc test actually use the imported types:

```rust
//! ```
//! use component_core::prelude::*;
//!
//! let registry = ComponentRegistry::new();
//! assert!(registry.list().is_empty());
//! ```
```

**Rationale**: Minor gap. Easy fix.

**Confidence**: HIGH

**Action**:

- File: `crates/component-core/src/prelude.rs` — improve doc test
- File: `crates/component-core/src/component.rs` — add doc test for `InterfaceMap::info()`

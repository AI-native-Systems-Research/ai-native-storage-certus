# Spec Conflict Report

Generated: 2026-04-15T12:00:00Z
Project: component-framework

## Summary

| Conflict Type | Count |
|---------------|-------|
| Same Feature, Different Behavior | 0 |
| Obsolete Constraints | 2 |
| Scope Overlap | 0 |
| Implicit Conflicts | 3 |
| **Total** | **5** |

All 5 findings are low-severity documentation inconsistencies between spec/plan/contract artifacts and the actual implementation. No blocking contradictions exist between specs. The implementation itself is consistent and correct.

---

## Conflicts

### Conflict 1: `define_actor!` macro planned but not implemented

**Type**: Obsolete Constraint
**Severity**: Low (documentation only)

**Sources**:
- `specs/003-actor-channels/plan.md`
- `specs/003-actor-channels/contracts/public-api.md`

**Description**:
The plan and contract documents for spec 003 reference a `define_actor!` macro that would generate `IUnknown` implementations for actor components, mirroring `define_component!`. This macro was never implemented. Instead, `Actor<M, H>` implements `IUnknown` directly in `crates/component-core/src/actor.rs:800-835`.

**Evidence**:

From `specs/003-actor-channels/contracts/public-api.md`:
> `define_actor!` macro ... generates `IUnknown` implementation

From implementation: `define_actor!` does not exist. `Actor<M, H>` has a hand-written `IUnknown` impl that correctly provides `ISender<M>` as its queryable interface.

**Impact**: None on implementation. The hand-written impl is fully functional, passes all tests, and supports third-party binding via `connect_receptacle_raw`. The contract is simply outdated relative to the implementation approach.

**Suggested Resolution**: SUPERSEDE — update `specs/003-actor-channels/contracts/public-api.md` to document the direct `IUnknown` impl approach instead of the macro approach.

**Action Required**:
- [ ] Update 003 contract to reflect direct `IUnknown` implementation on `Actor<M, H>`

---

### Conflict 2: `send_any`/`recv_any` type-erased channel methods planned but not implemented

**Type**: Obsolete Constraint
**Severity**: Low (documentation only)

**Sources**:
- `specs/003-actor-channels/contracts/public-api.md`

**Description**:
The contract for spec 003 references `send_any`/`recv_any` methods using `Box<dyn Any + Send>` for type-erased message passing at the `IUnknown` boundary. These methods do not exist in the implementation. Instead, channels use fully generic `ISender<T>` / `IReceiver<T>` traits with `TypeId`-based lookup through `IUnknown::query_interface_raw`.

**Evidence**:

From `specs/003-actor-channels/contracts/public-api.md`:
> `send_any`/`recv_any` use `Box<dyn Any + Send>`

From implementation: No `send_any` or `recv_any` exists anywhere in the codebase. `ISender<T>::send()` and `IReceiver<T>::recv()` are the actual API, accessed via typed `query()` calls.

**Impact**: None. The generic approach is type-safe and avoids the runtime downcast failure path that `send_any`/`recv_any` would have introduced.

**Suggested Resolution**: SUPERSEDE — update the contract to document the generic `ISender<T>` / `IReceiver<T>` approach.

**Action Required**:
- [ ] Remove `send_any`/`recv_any` references from 003 contract

---

### Conflict 3: Benchmark naming example uses `capacity_4096` not in parameter set

**Type**: Implicit Conflict
**Severity**: Minimal (documentation inconsistency)

**Sources**:
- `specs/004-channel-benchmarks/contracts/benchmark-suite.md`

**Description**:
The benchmark contract defines queue capacity parameters as `{64, 1024, 16384}` but includes a naming example using `capacity_4096`, which is not in the parameter set.

**Evidence**:

From `specs/004-channel-benchmarks/contracts/benchmark-suite.md:76`:
> `mpsc/crossbeam_bounded/vec1024/capacity_4096/producers_4`

But the defined capacity set is `{64, 1024, 16384}`.

**Impact**: Cosmetic. The actual benchmarks use the correct capacity values.

**Suggested Resolution**: MERGE — fix the example to use `capacity_1024` or `capacity_16384`.

**Action Required**:
- [ ] Fix benchmark naming example in 004 contract

---

### Conflict 4: No `From<ReceptacleError> for RegistryError` conversion

**Type**: Implicit Conflict
**Severity**: Low (ergonomic, not functional)

**Sources**:
- `specs/001-com-component-framework/spec.md` (defines `ReceptacleError`)
- `specs/002-registry-refcount-binding/spec.md` (defines `RegistryError`)

**Description**:
`Receptacle::connect()` returns `ReceptacleError`, while `IUnknown::connect_receptacle_raw()` returns `RegistryError`. The `connect_receptacle_raw` implementations must manually convert between these error types. No `From<ReceptacleError> for RegistryError` impl exists.

**Evidence**:

In `define_component!`-generated code and manual `connect_receptacle_raw` impls:
```rust
self.greeter.connect(arc).map_err(|e| RegistryError::BindingFailed {
    detail: e.to_string(),
})
```

**Impact**: Low. Every `connect_receptacle_raw` implementation must include a `.map_err()` conversion. This is a minor ergonomic burden but does not cause correctness issues.

**Suggested Resolution**: HUMAN_REQUIRED — decide whether to add `From<ReceptacleError> for RegistryError` or keep the explicit conversion.

**Action Required**:
- [ ] Consider adding `From<ReceptacleError> for RegistryError` impl

---

### Conflict 5: IUnknown trait extended without documenting cross-spec impact

**Type**: Implicit Conflict
**Severity**: Low (documentation only)

**Sources**:
- `specs/001-com-component-framework/spec.md` (defines IUnknown with 4 methods)
- `specs/002-registry-refcount-binding/spec.md` (adds `connect_receptacle_raw` to IUnknown)

**Description**:
Spec 001 defines `IUnknown` with 4 methods: `query_interface_raw`, `version`, `provided_interfaces`, `receptacles`. Spec 002 adds a 5th method (`connect_receptacle_raw`) to the trait. Spec 001 does not reference this extension, and spec 002's plan claims "backward compatibility" without noting that all `IUnknown` implementors must add the new method.

**Impact**: None in practice — both specs were implemented together, so no code was broken. However, the specs read as if they are independent and sequential, which could mislead a reader about the trait's actual shape.

**Suggested Resolution**: MERGE — add a note to spec 001 that `IUnknown` was extended by spec 002, or update spec 001's IUnknown definition to include all 5 methods.

**Action Required**:
- [ ] Add cross-reference between spec 001 and 002 regarding `connect_receptacle_raw`

---

## Inter-Spec Dependency Summary

All specs build on prior specs in a well-defined order:

```
001 (core framework)
 └── 002 (registry, refcount, binding)
      └── 003 (actors, channels)
           ├── 004 (channel benchmarks)
           ├── 005 (NUMA awareness)
           └── 006 (log handler)
```

No circular dependencies. No conflicting ownership of features.

---

## Resolution Tracking

| # | Conflict | Resolution | Decided By | Date |
|---|----------|------------|------------|------|
| 1 | `define_actor!` not implemented | SUPERSEDE | pending | - |
| 2 | `send_any`/`recv_any` not implemented | SUPERSEDE | pending | - |
| 3 | Benchmark capacity_4096 naming | MERGE | pending | - |
| 4 | No From<ReceptacleError> conversion | HUMAN_REQUIRED | pending | - |
| 5 | IUnknown cross-spec extension | MERGE | pending | - |

## Recommendations

1. **Update spec 003 contract** to reflect the actual implementation approach (direct `IUnknown` impl on `Actor`, no `define_actor!` macro, no `send_any`/`recv_any`). These are the two most significant documentation gaps.
2. **Fix the benchmark naming example** in spec 004 contract (trivial edit).
3. **Add cross-references** between specs 001 and 002 noting the `IUnknown` trait extension.
4. **Optionally** add `From<ReceptacleError> for RegistryError` to reduce boilerplate in `connect_receptacle_raw` implementations.

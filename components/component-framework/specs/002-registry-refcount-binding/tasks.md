# Tasks: Registry, Reference Counting, and Binding

**Input**: Design documents from `/specs/002-registry-refcount-binding/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/public-api.md

**Tests**: Required by constitution (Principles I–III) and spec (SC-002, SC-003, SC-007). TDD workflow: write failing tests first, then implement.

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story (US1, US2, US3, US4)
- All paths relative to workspace root

## Path Conventions

```text
crates/
├── component-core/src/             # Core traits and types (extended)
│   ├── component_ref.rs            # NEW: ComponentRef wrapper
│   ├── registry.rs                 # NEW: ComponentRegistry + ComponentFactory
│   ├── binding.rs                  # NEW: bind() free function
│   ├── error.rs                    # Extended with RegistryError
│   └── iunknown.rs                 # Extended with connect_receptacle_raw
├── component-macros/src/           # Procedural macros (extended)
│   └── define_component.rs         # Extended: connect_receptacle_raw codegen
└── component-framework/
    ├── src/lib.rs                  # Facade re-exports (extended)
    ├── tests/                      # Integration tests
    └── benches/                    # Criterion benchmarks
```

---

## Phase 1: Setup

**Purpose**: New module scaffolding and error type extension

- [X] T001 Create crates/component-core/src/component_ref.rs with module declaration and add `pub mod component_ref` to crates/component-core/src/lib.rs
- [X] T002 [P] Create crates/component-core/src/registry.rs with module declaration and add `pub mod registry` to crates/component-core/src/lib.rs
- [X] T003 [P] Create crates/component-core/src/binding.rs with module declaration and add `pub mod binding` to crates/component-core/src/lib.rs
- [X] T004 Extend RegistryError enum with NotFound, AlreadyRegistered, FactoryFailed, BindingFailed variants (Display, Error, Debug, Clone, PartialEq, Eq) in crates/component-core/src/error.rs

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: ComponentRef — the handle type that all user stories depend on

**CRITICAL**: US1 (registry) and US3 (binding) both return/consume ComponentRef, so it must exist first.

### Tests for ComponentRef

- [X] T005 [P] Unit test: ComponentRef wraps Arc and provides IUnknown access in crates/component-core/src/component_ref.rs
- [X] T006 [P] Unit test: attach() clones the handle (Arc count increments) in crates/component-core/src/component_ref.rs
- [X] T007 [P] Unit test: release (drop) decrements Arc count, component destroyed at zero in crates/component-core/src/component_ref.rs
- [X] T008 [P] Unit test: ComponentRef is Send + Sync in crates/component-core/src/component_ref.rs
- [X] T009 [P] Unit test: ComponentRef implements Clone (same as attach) in crates/component-core/src/component_ref.rs
- [X] T010 [P] Unit test: RegistryError Display impls in crates/component-core/src/error.rs

### Implementation for ComponentRef

- [X] T011 Implement ComponentRef struct (newtype over Arc<dyn IUnknown>) with attach(), Deref to dyn IUnknown, Clone, From<Arc<T>> where T: IUnknown, Send + Sync in crates/component-core/src/component_ref.rs
- [X] T012 Doc tests for ComponentRef (new, attach, deref, From conversion) in crates/component-core/src/component_ref.rs
- [X] T013 Wire ComponentRef re-export in crates/component-core/src/lib.rs and crates/component-framework/src/lib.rs

**Checkpoint**: ComponentRef compiles, wraps Arc, provides IUnknown access. All foundational tests pass.

---

## Phase 3: User Story 1 — Component Registry with Factory Instantiation (Priority: P1) MVP

**Goal**: Register factories by name, create components on demand. Multiple independent registries.

**Independent Test**: Register a factory, create a component by name, verify it implements expected interfaces.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T014 [P] [US1] Unit test: ComponentRegistry::new() creates empty registry in crates/component-core/src/registry.rs
- [X] T015 [P] [US1] Unit test: register and create lifecycle in crates/component-core/src/registry.rs
- [X] T016 [P] [US1] Unit test: create returns NotFound for unregistered name in crates/component-core/src/registry.rs
- [X] T017 [P] [US1] Unit test: register returns AlreadyRegistered for duplicate name in crates/component-core/src/registry.rs
- [X] T018 [P] [US1] Unit test: list() returns all registered names in crates/component-core/src/registry.rs
- [X] T019 [P] [US1] Unit test: unregister removes factory in crates/component-core/src/registry.rs
- [X] T020 [P] [US1] Unit test: create with typed config (Option<&dyn Any>) in crates/component-core/src/registry.rs
- [X] T021 [P] [US1] Unit test: registry is Send + Sync in crates/component-core/src/registry.rs
- [X] T022 [US1] Integration test: register, create, query interface end-to-end in crates/component-framework/tests/registry.rs
- [X] T023 [US1] Integration test: concurrent registry access from multiple threads in crates/component-framework/tests/registry.rs
- [X] T024 [US1] Integration test: factory panic is caught and returns FactoryFailed error in crates/component-framework/tests/registry.rs

### Implementation for User Story 1

- [X] T025 [US1] Implement ComponentFactory trait (create method with Option<&dyn Any> config, returning Result<ComponentRef, RegistryError>) in crates/component-core/src/registry.rs
- [X] T026 [US1] Implement blanket impl of ComponentFactory for Fn(Option<&dyn Any>) -> Result<ComponentRef, RegistryError> in crates/component-core/src/registry.rs
- [X] T027 [US1] Implement ComponentRegistry struct (RwLock<HashMap<String, Box<dyn ComponentFactory>>>) with new(), register(), unregister(), create(), list() in crates/component-core/src/registry.rs
- [X] T028 [US1] Implement panic-catching in create() via std::panic::catch_unwind in crates/component-core/src/registry.rs
- [X] T029 [US1] Doc tests for ComponentFactory, ComponentRegistry (register, create, list, error cases) in crates/component-core/src/registry.rs
- [X] T030 [US1] Wire registry re-exports (ComponentFactory, ComponentRegistry) in crates/component-core/src/lib.rs and crates/component-framework/src/lib.rs

**Checkpoint**: Registry works — register, create, list, unregister, error handling, thread safety. All US1 tests pass.

---

## Phase 4: User Story 2 — Atomic Reference Counting with Attach/Release (Priority: P1)

**Goal**: Verify ComponentRef reference counting semantics work correctly across threads.

**Independent Test**: Create a component, attach multiple references, release one by one, verify component destroyed at zero.

### Tests for User Story 2

> **NOTE: These tests exercise ComponentRef lifetime semantics built in Phase 2**

- [X] T031 [P] [US2] Integration test: attach increments count, release decrements, component destroyed at zero in crates/component-framework/tests/component_ref.rs
- [X] T032 [P] [US2] Integration test: concurrent attach/release across threads produces correct final state in crates/component-framework/tests/component_ref.rs
- [X] T033 [P] [US2] Integration test: ComponentRef from registry has initial count of 1 in crates/component-framework/tests/component_ref.rs
- [X] T034 [US2] Integration test: receptacle holds Arc, component stays alive after all ComponentRefs dropped in crates/component-framework/tests/component_ref.rs

### Implementation for User Story 2

- [X] T035 [US2] Add Arc::strong_count-based assertions to ComponentRef tests to verify exact reference counting behavior in crates/component-core/src/component_ref.rs
- [X] T036 [US2] Doc tests demonstrating attach/release lifecycle with count verification in crates/component-core/src/component_ref.rs

**Checkpoint**: Reference counting is verified — attach increments, drop decrements, zero count destroys, thread-safe. All US2 tests pass.

---

## Phase 5: User Story 3 — First-Party and Third-Party Binding (Priority: P2)

**Goal**: Third-party assembler can wire components by string names without knowing concrete types.

**Independent Test**: Wire two components using bind() with string names, verify method dispatch works.

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T037 [P] [US3] Integration test: first-party binding (direct receptacle connect) still works unchanged in crates/component-framework/tests/binding.rs
- [X] T038 [P] [US3] Integration test: third-party bind() wires two components by interface/receptacle names in crates/component-framework/tests/binding.rs
- [X] T039 [P] [US3] Integration test: bind() returns error for unknown interface name in crates/component-framework/tests/binding.rs
- [X] T040 [P] [US3] Integration test: bind() returns error for unknown receptacle name in crates/component-framework/tests/binding.rs
- [X] T041 [P] [US3] Integration test: bind() returns error for type-mismatched interface/receptacle in crates/component-framework/tests/binding.rs
- [X] T042 [US3] Integration test: third-party assembler wires multiple receptacles on one component in crates/component-framework/tests/binding.rs

### Implementation for User Story 3

- [X] T043 [US3] Add connect_receptacle_raw method to IUnknown trait definition in crates/component-core/src/iunknown.rs
- [X] T044 [US3] Generate connect_receptacle_raw implementation in define_component! macro (match on receptacle name, downcast Box<dyn Any> to Arc<dyn IFoo>, call connect) in crates/component-macros/src/define_component.rs
- [X] T045 [US3] Implement bind() free function (resolve interface name → TypeId → query → box → connect_receptacle_raw) in crates/component-core/src/binding.rs
- [X] T046 [US3] Unit tests for bind() error paths in crates/component-core/src/binding.rs
- [X] T047 [US3] Doc tests for bind() showing third-party wiring in crates/component-core/src/binding.rs
- [X] T048 [US3] Doc tests for connect_receptacle_raw in crates/component-core/src/iunknown.rs
- [X] T049 [US3] Wire binding re-exports (bind) in crates/component-core/src/lib.rs and crates/component-framework/src/lib.rs

**Checkpoint**: Both binding modes work — first-party (direct connect) and third-party (bind by name). Type mismatches caught. All US3 tests pass.

---

## Phase 6: User Story 4 — Registry-Driven Assembly (Priority: P3)

**Goal**: End-to-end pipeline: registry create → third-party bind → cross-component method dispatch.

**Independent Test**: Register factories, create by name, wire via bind(), invoke operations across component boundaries.

### Tests for User Story 4

- [X] T050 [P] [US4] Integration test: create two components from registry, bind by names, invoke cross-component method in crates/component-framework/tests/assembly.rs
- [X] T051 [P] [US4] Integration test: multi-component assembly (3+ components with chained wiring) in crates/component-framework/tests/assembly.rs
- [X] T052 [US4] Integration test: release all ComponentRefs after assembly, verify clean destruction in crates/component-framework/tests/assembly.rs

### Implementation for User Story 4

- [X] T053 [US4] No new code needed — this story validates integration of US1+US2+US3. Ensure all assembly tests pass.

**Checkpoint**: Full pipeline works — registry → create → bind → invoke → release. All US4 tests pass.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Benchmarks, documentation completeness, CI validation

- [X] T054 [P] Criterion benchmark: registry create latency in crates/component-framework/benches/registry.rs
- [X] T055 [P] Criterion benchmark: bind() latency in crates/component-framework/benches/binding.rs
- [X] T056 [P] Criterion benchmark: ComponentRef attach/release latency in crates/component-framework/benches/component_ref.rs
- [X] T057 [P] Crate-level documentation updates for new modules in crates/component-core/src/lib.rs
- [X] T058 Run `cargo doc --no-deps` and fix all warnings across workspace
- [X] T059 Run full CI gate: `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps`
- [X] T060 Validate quickstart.md examples compile and pass as integration tests

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories (ComponentRef needed everywhere)
- **US1 (Phase 3)**: Depends on Phase 2 — registry returns ComponentRef
- **US2 (Phase 4)**: Depends on Phase 2 — tests ComponentRef lifecycle. Independent of US1.
- **US3 (Phase 5)**: Depends on Phase 2 — REQUIRES extending IUnknown and define_component! macro. Independent of US1 and US2.
- **US4 (Phase 6)**: Depends on US1 + US3 (needs registry + binding)
- **Polish (Phase 7)**: Depends on all user stories

### User Story Dependencies

```text
Phase 1 (Setup) → Phase 2 (Foundational: ComponentRef)
                        │
             ┌──────────┼──────────┐
             ▼          ▼          ▼
        Phase 3    Phase 4    Phase 5
        (US1:      (US2:      (US3:
        Registry)  RefCount)  Binding)
             │                     │
             └──────────┬──────────┘
                        ▼
                   Phase 6
                   (US4: Assembly)
                        │
                        ▼
                   Phase 7 (Polish)
```

### Within Each User Story

1. Tests MUST be written and FAIL before implementation
2. Core types before services/logic
3. Unit tests before integration tests
4. Doc tests after implementation is stable
5. Verify all story tests pass before moving to next phase

### Parallel Opportunities

- Phase 1: T002, T003 can run in parallel
- Phase 2: T005–T010 (tests) can all run in parallel
- Phase 3: T014–T021 (unit tests) can all run in parallel
- Phase 4: T031–T033 can run in parallel
- Phase 5: T037–T041 (integration tests) can all run in parallel
- Phase 6: T050, T051 can run in parallel
- Phase 7: T054, T055, T056, T057 can all run in parallel
- US1, US2, US3 can be developed in parallel after Phase 2

---

## Implementation Strategy

### MVP First (US1 + US2)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (ComponentRef)
3. Complete Phase 3: US1 (Registry)
4. Complete Phase 4: US2 (Reference Counting verification)
5. **STOP and VALIDATE**: Components can be created by name, reference counting is correct
6. This delivers SC-001, SC-002, SC-003

### Incremental Delivery

1. Setup + Foundational → ComponentRef compiles, wraps Arc
2. US1 → Registry creates components by name → **MVP!**
3. US2 → Reference counting verified across threads → **Lifecycle safe**
4. US3 → Third-party binding by string names → **Full framework**
5. US4 → End-to-end assembly validation → **Integration complete**
6. Polish → Benchmarks, docs, CI gate → **Production ready**

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- US1 and US2 are both P1 but independent of each other (both depend only on ComponentRef)
- US3 is independent of US1 and US2; US4 depends on US1 + US3
- Constitution requires: doc tests on all public APIs, Criterion benchmarks on perf-sensitive code, clippy clean, zero cargo doc warnings
- Commit after each task or logical group
- Stop at any checkpoint to validate independently
- Existing feature 001 tests MUST continue to pass throughout — backward compatibility required

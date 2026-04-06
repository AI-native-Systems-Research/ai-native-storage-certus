# Tasks: COM-Style Component Framework

**Input**: Design documents from `/specs/001-com-component-framework/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/public-api.md

**Tests**: Required by constitution (Principles I–III) and spec (SC-004, SC-005). TDD workflow: write failing tests first, then implement.

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story (US1, US2, US3, US4)
- All paths relative to workspace root

## Path Conventions

```text
Cargo.toml                          # Workspace root
crates/
├── component-core/src/             # Core traits and types
├── component-macros/src/           # Procedural macros
└── component-framework/src/        # Facade re-exports
benches/                            # Criterion benchmarks
tests/                              # Integration tests
```

---

## Phase 1: Setup

**Purpose**: Workspace initialization and crate scaffolding

- [X] T001 Create workspace root Cargo.toml with members: crates/component-core, crates/component-macros, crates/component-framework; add criterion dev-dependency
- [X] T002 [P] Create crates/component-core/Cargo.toml (lib crate, edition 2021) and crates/component-core/src/lib.rs with module declarations
- [X] T003 [P] Create crates/component-macros/Cargo.toml (proc-macro crate, depends on syn, quote, proc-macro2, component-core) and crates/component-macros/src/lib.rs
- [X] T004 [P] Create crates/component-framework/Cargo.toml (depends on component-core + component-macros) and crates/component-framework/src/lib.rs with re-exports
- [X] T005 [P] Add rustfmt.toml and clippy.toml at workspace root; verify `cargo fmt --check && cargo clippy -- -D warnings` passes on empty workspace

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and traits that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [X] T006 [P] Implement ReceptacleError and QueryError enums with Display, Error, Debug, Clone, PartialEq, Eq derives and doc comments in crates/component-core/src/error.rs
- [X] T007 [P] Implement Interface marker trait (Send + Sync + 'static) in crates/component-core/src/interface.rs
- [X] T008 [P] Implement InterfaceInfo struct (type_id: TypeId, name: &'static str) with Debug, Clone derives and doc comments in crates/component-core/src/interface.rs
- [X] T009 [P] Implement ReceptacleInfo struct (type_id: TypeId, name: &'static str, interface_name: &'static str) with Debug, Clone derives and doc comments in crates/component-core/src/interface.rs
- [X] T010 Implement InterfaceMap struct (HashMap<TypeId, Box<dyn Any + Send + Sync>>, Vec<InterfaceInfo>) with insert and lookup methods in crates/component-core/src/component.rs
- [X] T011 Implement IUnknown trait definition (query_interface_raw, version, provided_interfaces, receptacles) in crates/component-core/src/iunknown.rs
- [X] T012 Implement query::<I>() free function (typed wrapper over query_interface_raw with downcast) in crates/component-core/src/iunknown.rs
- [X] T013 Wire all module re-exports in crates/component-core/src/lib.rs (pub use error, interface, iunknown, component modules)
- [X] T014 [P] Unit tests for InterfaceMap insert/lookup in crates/component-core/src/component.rs
- [X] T015 [P] Unit tests for error Display impls in crates/component-core/src/error.rs
- [X] T016 [P] Doc tests for InterfaceInfo, ReceptacleInfo, ReceptacleError, QueryError in their respective files

**Checkpoint**: Foundation ready — all core types compile and pass unit tests

---

## Phase 3: User Story 1 — Define an Interface with Macros (Priority: P1) MVP

**Goal**: Library authors can define interfaces with `define_interface!` that generate traits usable across crates without implementation access.

**Independent Test**: Define an interface with the macro, compile a separate crate that depends only on the interface definition, and verify the trait is usable as a type bound.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T017 [P] [US1] Integration test: define_interface! generates valid trait with methods in tests/interface_definition.rs
- [X] T018 [P] [US1] Integration test: interface usable as type bound in separate compilation unit in tests/cross_crate_isolation.rs
- [X] T019 [P] [US1] Integration test: define_interface! with lifetime parameters compiles in tests/interface_definition.rs
- [X] T020 [P] [US1] Compile-fail test: malformed macro input produces clear error (use trybuild crate) in tests/interface_definition.rs

### Implementation for User Story 1

- [X] T021 [US1] Implement define_interface! input parsing (interface name, method signatures with lifetimes) using syn in crates/component-macros/src/define_interface.rs
- [X] T022 [US1] Implement define_interface! code generation: trait definition with Send + Sync + 'static bounds, Interface impl, InterfaceInfo const in crates/component-macros/src/define_interface.rs
- [X] T023 [US1] Add Span-based compile-time error diagnostics for missing methods, invalid signatures in crates/component-macros/src/define_interface.rs
- [X] T024 [US1] Register define_interface! proc macro entry point in crates/component-macros/src/lib.rs
- [X] T025 [US1] Wire define_interface! re-export in crates/component-framework/src/lib.rs
- [X] T026 [US1] Doc tests for define_interface! showing basic usage and lifetime usage in crates/component-macros/src/lib.rs

**Checkpoint**: `define_interface!` works, generates valid traits, supports lifetimes, produces clear errors. All US1 tests pass.

---

## Phase 4: User Story 2 — Implement a Component with IUnknown (Priority: P1)

**Goal**: Developers create components with `define_component!` that implement IUnknown for interface query, version, and enumeration.

**Independent Test**: Instantiate a component, call IUnknown methods to query interfaces and version, verify correct results.

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T027 [P] [US2] Integration test: query_interface returns valid Arc for provided interface in tests/component_iunknown.rs
- [X] T028 [P] [US2] Integration test: query_interface returns None for unsupported interface in tests/component_iunknown.rs
- [X] T029 [P] [US2] Integration test: version() returns declared version string in tests/component_iunknown.rs
- [X] T030 [P] [US2] Integration test: query() typed free function works through dyn IUnknown in tests/component_iunknown.rs

### Implementation for User Story 2

- [X] T031 [US2] Implement define_component! input parsing (name, version, provides list, optional receptacles, optional fields) using syn in crates/component-macros/src/define_component.rs
- [X] T032 [US2] Implement define_component! code generation: struct with InterfaceMap, version, user fields; new() constructor that populates InterfaceMap in crates/component-macros/src/define_component.rs
- [X] T033 [US2] Generate IUnknown impl (query_interface_raw via InterfaceMap lookup, version, provided_interfaces, receptacles) in crates/component-macros/src/define_component.rs
- [X] T034 [US2] Add Span-based error diagnostics for define_component! (missing version, invalid provides list) in crates/component-macros/src/define_component.rs
- [X] T035 [US2] Register define_component! proc macro entry point in crates/component-macros/src/lib.rs
- [X] T036 [US2] Wire define_component! re-export in crates/component-framework/src/lib.rs
- [X] T037 [US2] Doc tests for define_component! and IUnknown::query_interface_raw in crates/component-macros/src/lib.rs and crates/component-core/src/iunknown.rs

**Checkpoint**: `define_component!` generates working components with IUnknown. Interface query, version, and enumeration all work. All US2 tests pass.

---

## Phase 5: User Story 3 — Connect Receptacles Between Components (Priority: P2)

**Goal**: Integrators wire a component's receptacle to another component's provided interface for cross-component method dispatch.

**Independent Test**: Create two components (one providing ILogger, one requiring ILogger), wire the receptacle, verify method dispatch works.

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T038 [P] [US3] Unit test: Receptacle::new() creates disconnected receptacle in crates/component-core/src/receptacle.rs
- [X] T039 [P] [US3] Unit test: connect/disconnect/get lifecycle in crates/component-core/src/receptacle.rs
- [X] T040 [P] [US3] Unit test: connect on already-connected returns AlreadyConnected error in crates/component-core/src/receptacle.rs
- [X] T041 [P] [US3] Unit test: get on disconnected returns NotConnected error in crates/component-core/src/receptacle.rs
- [X] T042 [P] [US3] Unit test: Receptacle is Send + Sync in crates/component-core/src/receptacle.rs
- [X] T043 [US3] Integration test: two-component wiring with method dispatch in tests/receptacle_wiring.rs

### Implementation for User Story 3

- [X] T044 [US3] Implement Receptacle<T> struct (RwLock<Option<Arc<T>>>) with new(), connect(), disconnect(), is_connected(), get() in crates/component-core/src/receptacle.rs
- [X] T045 [US3] Add receptacle field generation to define_component! macro (parse receptacles block, generate Receptacle<dyn IFoo> fields, populate ReceptacleInfo) in crates/component-macros/src/define_component.rs
- [X] T046 [US3] Wire Receptacle re-export in crates/component-core/src/lib.rs
- [X] T047 [US3] Doc tests for Receptacle (new, connect, disconnect, get, error cases) in crates/component-core/src/receptacle.rs

**Checkpoint**: Receptacles connect/disconnect/dispatch correctly. Type safety enforced at compile time. All US3 tests pass.

---

## Phase 6: User Story 4 — Introspect Component Capabilities (Priority: P3)

**Goal**: Runtime tools enumerate all interfaces and receptacles of a component for validation, wiring diagrams, or documentation.

**Independent Test**: Instantiate a multi-interface component, enumerate via IUnknown, verify lists match declaration.

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T048 [P] [US4] Integration test: provided_interfaces() returns complete list including IUnknown in tests/component_iunknown.rs
- [X] T049 [P] [US4] Integration test: receptacles() returns list with correct names and interface types in tests/component_iunknown.rs
- [X] T050 [US4] Integration test: multi-interface component (3+ interfaces) enumeration in tests/component_iunknown.rs

### Implementation for User Story 4

- [X] T051 [US4] Ensure define_component! codegen includes IUnknown in provided_interfaces() list in crates/component-macros/src/define_component.rs
- [X] T052 [US4] Ensure define_component! codegen populates ReceptacleInfo with correct names and interface_name fields in crates/component-macros/src/define_component.rs
- [X] T053 [US4] Doc tests for provided_interfaces() and receptacles() with multi-interface example in crates/component-core/src/iunknown.rs

**Checkpoint**: All introspection queries return complete, accurate metadata. All US4 tests pass.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Benchmarks, documentation completeness, CI validation

- [X] T054 [P] Criterion benchmark: query_interface latency (target ≤50 ns) in benches/query_interface.rs
- [X] T055 [P] Criterion benchmark: receptacle connect/disconnect/get latency in benches/receptacle.rs
- [X] T056 [P] Criterion benchmark: interface method dispatch overhead vs direct trait call (target ≤10 ns) in benches/method_dispatch.rs
- [X] T057 [P] Crate-level documentation (//! doc comments) in crates/component-core/src/lib.rs with quick-start and module overview
- [X] T058 [P] Crate-level documentation (//! doc comments) in crates/component-macros/src/lib.rs with macro usage examples
- [X] T059 [P] Crate-level documentation (//! doc comments) in crates/component-framework/src/lib.rs with unified quick-start
- [X] T060 Run `cargo doc --no-deps` and fix all warnings across workspace
- [X] T061 Run full CI gate: `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps`
- [X] T062 Validate quickstart.md examples compile and pass as integration tests

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 — BLOCKS US2 (components need interfaces)
- **US2 (Phase 4)**: Depends on US1 — BLOCKS US3 and US4 (receptacles and introspection need components)
- **US3 (Phase 5)**: Depends on US2 — independent of US4
- **US4 (Phase 6)**: Depends on US2 — independent of US3
- **Polish (Phase 7)**: Depends on US1–US4 completion

### User Story Dependencies

```text
Phase 1 (Setup) → Phase 2 (Foundational)
                        │
                        ▼
                   Phase 3 (US1: Interfaces)
                        │
                        ▼
                   Phase 4 (US2: Components + IUnknown)
                      │           │
                      ▼           ▼
    Phase 5 (US3: Receptacles)   Phase 6 (US4: Introspection)
                      │           │
                      ▼           ▼
                   Phase 7 (Polish)
```

### Within Each User Story

1. Tests MUST be written and FAIL before implementation
2. Core types/parsing before code generation
3. Code generation before registration and re-exports
4. Doc tests after implementation is stable
5. Verify all story tests pass before moving to next phase

### Parallel Opportunities

- Phase 1: T002, T003, T004, T005 can all run in parallel
- Phase 2: T006, T007, T008, T009 can run in parallel; T014, T015, T016 can run in parallel
- Phase 3: T017, T018, T019, T020 (tests) can run in parallel
- Phase 4: T027, T028, T029, T030 (tests) can run in parallel
- Phase 5: T038–T042 (unit tests) can run in parallel
- Phase 6: T048, T049 can run in parallel
- Phase 7: T054, T055, T056, T057, T058, T059 can all run in parallel

---

## Parallel Example: Phase 2 (Foundational)

```text
# Launch all independent type definitions together:
Task T006: "Implement error enums in crates/component-core/src/error.rs"
Task T007: "Implement Interface trait in crates/component-core/src/interface.rs"
Task T008: "Implement InterfaceInfo in crates/component-core/src/interface.rs"
Task T009: "Implement ReceptacleInfo in crates/component-core/src/interface.rs"

# Then sequential:
Task T010: "Implement InterfaceMap" (depends on T007, T008)
Task T011: "Implement IUnknown trait" (depends on T008, T009)
Task T012: "Implement query() function" (depends on T011)
Task T013: "Wire re-exports" (depends on all above)

# Launch all unit tests together:
Task T014: "Unit tests for InterfaceMap"
Task T015: "Unit tests for error Display"
Task T016: "Doc tests for core types"
```

---

## Implementation Strategy

### MVP First (US1 + US2)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational
3. Complete Phase 3: US1 (define_interface!)
4. Complete Phase 4: US2 (define_component! + IUnknown)
5. **STOP and VALIDATE**: Interfaces can be defined, components created, queries work
6. This delivers SC-001, SC-003, SC-004 partially

### Incremental Delivery

1. Setup + Foundational → crate structure compiles
2. US1 → interfaces defined via macro, cross-crate isolation works
3. US2 → components with IUnknown, interface query works → **MVP!**
4. US3 → receptacle wiring, cross-component composition → **Full framework**
5. US4 → introspection, enumeration → **Tooling-ready**
6. Polish → benchmarks validate performance targets, docs complete

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- US1 and US2 are both P1 but US2 depends on US1 (need interfaces to test components)
- US3 and US4 are independent of each other and can run in parallel after US2
- Constitution requires: doc tests on all public APIs, Criterion benchmarks on perf-sensitive code, clippy clean, zero cargo doc warnings
- Commit after each task or logical group
- Stop at any checkpoint to validate independently

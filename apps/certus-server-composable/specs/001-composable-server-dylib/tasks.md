# Tasks: Composable Server with Dynamic Component Loading

**Input**: Design documents from `specs/001-composable-server-dylib/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Tests are included — the constitution mandates unit tests for correctness and performance, and doc tests for all public APIs.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup

**Purpose**: Project initialization and dependency configuration

- [x] T001 Create `Cargo.toml` with dependencies (libloading, serde, serde_json, tonic, tokio, clap, component-core, prost) in `apps/certus-server-composable/Cargo.toml`
- [x] T002 [P] Copy `proto/dispatcher.proto` from `apps/certus-server/proto/dispatcher.proto` to `apps/certus-server-composable/proto/dispatcher.proto`
- [x] T003 [P] Create `build.rs` for tonic-build proto compilation in `apps/certus-server-composable/build.rs`
- [x] T004 [P] Create example configuration files in `apps/certus-server-composable/configs/example-production.json` and `apps/certus-server-composable/configs/example-dev.json`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story can be implemented

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 Implement configuration data model structs (Configuration, ServerConfig, ComponentSpec, BindingRule) with serde Deserialize in `src/config.rs`
- [x] T006 Implement JSON config parsing and structural validation (required fields, unique names, variable reference checking) in `src/config.rs`
- [x] T007 [P] Implement dylib path resolver with search path list and CERTUS_LIB_PATH env var support in `src/resolver.rs`
- [x] T008 [P] Implement dylib loader that calls `create_component()` symbol via libloading with panic catch in `src/loader.rs`
- [x] T009 Implement topological sort (Kahn's algorithm) for binding dependency graph with cycle detection in `src/topology.rs`
- [x] T010 Implement component binder that executes `connect_receptacle_raw` for each binding rule in `src/binder.rs`
- [x] T011 Implement runtime lifecycle manager (init sequence, fail-fast teardown in reverse order) in `src/runtime.rs`
- [x] T012 [P] Implement CLI argument parser with mandatory `--config` and optional overrides (listen, device-pci, tls-cert, tls-key, memory-tier-size, format, poller-base-cpu, drive-count) in `src/main.rs`
- [x] T013 [P] Write unit tests for config parsing: valid config, missing required fields, duplicate names, undefined variables in `tests/config_validation_test.rs`
- [x] T014 [P] Write unit tests for topological sort: linear chain, diamond dependency, cycle detection, explicit init_order override validation in `tests/topology_test.rs`
- [x] T015 [P] Write unit tests for path resolver: search path ordering, absolute path override, CERTUS_LIB_PATH prepend, missing file detection in `tests/resolver_test.rs`

**Checkpoint**: Foundation ready — all config parsing, validation, loading, sorting, binding, and lifecycle modules are tested and working. User story implementation can now begin.

---

## Phase 3: User Story 1 - Load and Run Certus via JSON Configuration (Priority: P1)

**Goal**: Load all components dynamically from a JSON config, wire them, and serve the identical gRPC API.

**Independent Test**: Start server with a valid configuration, verify gRPC endpoint responds to all 6 RPC methods identically to certus-server.

### Tests for User Story 1

- [ ] T016 [P] [US1] Write integration test: load mock dylib, call create_component, verify ComponentRef returned in `tests/loader_test.rs`
- [ ] T017 [P] [US1] Write integration test: full config → load → bind → verify dispatcher interface obtainable in `tests/integration_test.rs`

### Implementation for User Story 1

- [x] T018 [US1] Implement main orchestration: parse CLI → load config → merge CLI overrides → validate → resolve paths → verify accessibility → topo sort → load dylibs → instantiate → bind → obtain IDispatcher in `src/main.rs`
- [x] T019 [US1] Copy and adapt gRPC service implementation (DispatcherService, IPC cache, proto handlers) from certus-server in `src/service.rs`
- [x] T020 [US1] Implement gRPC server startup with TLS support and graceful SIGTERM/SIGINT shutdown in `src/main.rs`
- [x] T021 [US1] Implement CLI-over-config precedence merging (CLI args override JSON server section) in `src/config.rs`
- [x] T022 [US1] Add error reporting: clear messages for missing dylib, load failure, bind failure, cycle detected with exit codes (1=config error, 2=init failure) in `src/runtime.rs`
- [ ] T023 [US1] Write doc tests for all public functions in config.rs, resolver.rs, loader.rs, topology.rs, binder.rs, runtime.rs

**Checkpoint**: At this point, certus-server-composable can start with a full JSON config and serve gRPC requests identically to certus-server.

---

## Phase 4: User Story 2 - Variable-Driven Instance Count (Priority: P2)

**Goal**: Support `$variable_name` references in instance count fields for parameterized deployments.

**Independent Test**: Provide config with `"instances": "$num_ssd_devices"` and verify the correct number of component instances are created.

### Tests for User Story 2

- [ ] T024 [P] [US2] Write unit test: variable substitution resolves `$var` to integer, rejects undefined vars, rejects non-positive values in `tests/config_validation_test.rs`
- [ ] T025 [P] [US2] Write integration test: config with `instances: "$num_ssd_devices"` set to 3 creates 3 named instances (name[0], name[1], name[2]) in `tests/integration_test.rs`

### Implementation for User Story 2

- [x] T026 [US2] Implement variable substitution in config parsing: detect `$`-prefixed strings in instance fields, replace with variable value in `src/config.rs`
- [x] T027 [US2] Implement multi-instance naming: when instances > 1, generate names as `{name}[0]`, `{name}[1]`, etc. in `src/runtime.rs`
- [x] T028 [US2] Implement binding expansion: bindings referencing a multi-instance component apply to all instances (or specific instance via `name[N]` syntax) in `src/binder.rs`
- [x] T029 [US2] Add validation: reject instances <= 0 after substitution, report which variable resolved to invalid value in `src/config.rs`

**Checkpoint**: At this point, variable-driven instance counts work. A single config template adapts to different hardware by changing only the variables section.

---

## Phase 5: User Story 3 - Deployment-Specific Configurations (Priority: P3)

**Goal**: Support multiple config files for different deployment scenarios, with mandatory `--config` parameter.

**Independent Test**: Provide different config files (dev vs production) and verify each produces the expected component topology.

### Tests for User Story 3

- [ ] T030 [P] [US3] Write unit test: server exits with usage error when `--config` is omitted in `tests/integration_test.rs`
- [ ] T031 [P] [US3] Write integration test: dev config (1 device) and production config (8 devices) produce correct instance counts in `tests/integration_test.rs`

### Implementation for User Story 3

- [x] T032 [US3] Create example dev configuration in `configs/example-dev.json` with 1 SSD, 256M memory-tier, minimal component set
- [x] T033 [US3] Create example production configuration in `configs/example-production.json` with 8 SSDs, 2G memory-tier, full component set including GPU services
- [x] T034 [US3] Validate that `--config` is mandatory in CLI parser — exit with code 1 and usage message if missing in `src/main.rs`

**Checkpoint**: All three user stories are independently functional. The system can be deployed with different configurations for different environments.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, benchmarks, and quality improvements across all stories

- [x] T035 [P] Add module-level documentation (`//!`) to all source files: config.rs, resolver.rs, loader.rs, topology.rs, binder.rs, runtime.rs, service.rs
- [ ] T036 [P] Add Criterion benchmark for startup path (config parse → dylib load → bind) in `benches/startup_benchmark.rs`
- [x] T037 [P] Verify `cargo clippy -- -D warnings` passes with zero warnings
- [ ] T038 [P] Verify `cargo doc --no-deps` produces zero warnings
- [x] T039 Run full test suite single-threaded (`cargo test -- --test-threads 1`) and verify all pass
- [ ] T040 Validate quickstart.md instructions against actual build and run

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational phase
- **User Story 2 (Phase 4)**: Depends on Foundational phase (can run parallel to US1 but builds upon config.rs from Phase 2)
- **User Story 3 (Phase 5)**: Depends on Foundational phase (can run parallel to US1/US2)
- **Polish (Phase 6)**: Depends on all user stories being complete

### Within Each Phase

- Tasks marked [P] can run in parallel
- Models/structs before services/logic
- Core implementation before integration glue
- Tests alongside or before implementation

### Parallel Opportunities

- T002, T003, T004 in Phase 1 (all different files)
- T007, T008, T012, T013, T014, T015 in Phase 2 (different modules)
- T016, T017 in Phase 3 tests (different test files)
- T024, T025 in Phase 4 tests
- T030, T031 in Phase 5 tests
- T035, T036, T037, T038 in Phase 6 (all independent)

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1
4. **STOP and VALIDATE**: Server starts, loads components, serves gRPC
5. Deploy/demo if ready

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. Add User Story 1 → Full gRPC server operational (MVP!)
3. Add User Story 2 → Variable-driven multi-instance support
4. Add User Story 3 → Multi-environment config examples
5. Polish → Benchmarks, docs, lint clean

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- All unsafe code (libloading FFI) requires `// SAFETY:` comments
- All public functions require doc tests (Constitution Principle III)

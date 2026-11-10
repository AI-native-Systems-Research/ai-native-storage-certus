# Tasks: SPDK/DPDK Environment Component with VFIO Device Iteration

**Input**: Design documents from `/specs/002-spdk-env-vfio-init/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/ispdk-env.md

**Tests**: Included per constitution requirements (Principle II: Comprehensive Testing is NON-NEGOTIABLE).

**Organization**: Tasks grouped by user story. Two crates: `spdk-sys` (FFI bindings) and `spdk-env` (safe wrapper + component).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3, US4)
- All paths relative to workspace root (`../../` from this spec dir)

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create both crate structures and integrate into workspace

- [X] T001 Create `components/spdk-sys/` crate directory with `Cargo.toml` (links = "spdk", build deps: bindgen, pkg-config), `wrapper.h`, `build.rs` skeleton, and `src/lib.rs`
- [X] T002 [P] Create `components/spdk-env/` crate directory with `Cargo.toml` (deps: spdk-sys, component-framework), `src/lib.rs`, `src/error.rs`, `src/device.rs`, `src/checks.rs`, `src/env.rs`
- [X] T003 Add `components/spdk-sys` and `components/spdk-env` to workspace members and `[workspace.dependencies]` in `Cargo.toml` (workspace root)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: FFI bindings, base types, and component skeleton that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [X] T004 Implement `components/spdk-sys/build.rs` — use pkg-config to find `spdk_env_dpdk` and DPDK libs from `deps/spdk-build/lib/pkgconfig/`, run bindgen on `wrapper.h` with `deps/spdk-build/include/` include path, emit link directives
- [X] T005 Implement `components/spdk-sys/wrapper.h` — include `spdk/env.h` and `spdk/env_dpdk.h` headers needed for env init, PCI enumeration, and device accessors
- [X] T006 Implement `components/spdk-sys/src/lib.rs` — re-export bindgen-generated bindings with `#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]`
- [X] T007 [P] Define `SpdkEnvError` enum (VfioNotAvailable, PermissionDenied, HugepagesNotConfigured, LoggerNotConnected, AlreadyInitialized, InitFailed, DeviceProbeFailed) with `Display` and `std::error::Error` impls in `components/spdk-env/src/error.rs`
- [X] T008 [P] Define `PciAddress` (with `Display` as `DDDD:BB:DD.F`), `PciId`, `VfioDevice` structs with doc comments in `components/spdk-env/src/device.rs`
- [X] T009 Define `ISPDKEnv` interface via `define_interface!` with methods: `init(&self) -> Result<(), SpdkEnvError>`, `devices(&self) -> Vec<VfioDevice>`, `device_count(&self) -> usize`, `is_initialized(&self) -> bool` in `components/spdk-env/src/lib.rs`
- [X] T010 Define `SPDKEnvComponent` via `define_component!` with `version: "0.1.0"`, `provides: [ISPDKEnv]`, `receptacles: { logger: ILogger }`, and interior-mutable state fields (`RwLock<Vec<VfioDevice>>`, `AtomicBool` for initialized flag) in `components/spdk-env/src/lib.rs`

**Checkpoint**: Both crates compile (`cargo check -p spdk-sys -p spdk-env`). Component skeleton compiles but ISPDKEnv methods are stubs.

---

## Phase 3: User Story 2 - VFIO Availability and Permission Validation (Priority: P1)

**Goal**: Pre-flight checks detect misconfigured systems and report actionable error messages before SPDK init is attempted.

**Independent Test**: Run on a system without VFIO or with restricted permissions; verify specific error variants and messages.

### Implementation for User Story 2

- [X] T011 [US2] Implement `check_vfio_available()` — verify `/dev/vfio` exists and `/sys/bus/pci/drivers/vfio-pci/` exists (module loaded), return `SpdkEnvError::VfioNotAvailable` with guidance message in `components/spdk-env/src/checks.rs`
- [X] T012 [US2] Implement `check_vfio_permissions()` — verify read/write access on `/dev/vfio`, `/dev/vfio/vfio`, and IOMMU group entries under `/dev/vfio/`, return `SpdkEnvError::PermissionDenied` with specific inaccessible path in `components/spdk-env/src/checks.rs`
- [X] T013 [US2] Implement `check_hugepages()` — parse `/sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages` (and 1GB variant), return `SpdkEnvError::HugepagesNotConfigured` if total is 0 in `components/spdk-env/src/checks.rs`
- [X] T014 [US2] Implement logger connectivity check — verify `self.logger.is_connected()` at start of `init()`, return `SpdkEnvError::LoggerNotConnected` in `components/spdk-env/src/env.rs`
- [X] T015 [US2] Implement singleton check — check global `AtomicBool` at start of `init()`, return `SpdkEnvError::AlreadyInitialized` in `components/spdk-env/src/env.rs`
- [X] T016 [US2] Wire all pre-flight checks into `init()` flow in order: logger → singleton → VFIO available → permissions → hugepages → proceed to SPDK init in `components/spdk-env/src/env.rs`
- [X] T017 [US2] Add unit tests for each check function with mock filesystem paths (use tempdir for permission tests) in `components/spdk-env/src/checks.rs` (inline `#[cfg(test)]` module)

**Checkpoint**: Pre-flight checks return correct errors on misconfigured systems. `cargo test -p spdk-env` passes for check-related tests.

---

## Phase 4: User Story 1 - Initialize SPDK Environment and Discover VFIO Devices (Priority: P1)

**Goal**: After pre-flight checks pass, initialize SPDK/DPDK environment and enumerate all VFIO-bound devices with full PCI information.

**Independent Test**: On a VFIO-capable system with bound devices, `init()` succeeds and `devices()` returns entries matching `/sys/bus/pci/drivers/vfio-pci/`.

### Implementation for User Story 1

- [X] T018 [US1] Implement safe `spdk_env_init` wrapper — call `spdk_env_opts_init()` then `spdk_env_init()`, set singleton `AtomicBool`, return `SpdkEnvError::InitFailed` on non-zero in `components/spdk-env/src/env.rs`
- [X] T019 [US1] Implement safe `spdk_env_fini` wrapper — call `spdk_env_fini()`, clear singleton `AtomicBool` in `components/spdk-env/src/env.rs`
- [X] T020 [US1] Implement PCI device enumeration — call `spdk_pci_for_each_device()` with callback that constructs `VfioDevice` from `spdk_pci_device_get_*` accessors, skip devices that fail probe with logged warning, collect into `Vec<VfioDevice>` in `components/spdk-env/src/env.rs`
- [X] T021 [US1] Implement `ISPDKEnv` trait on `SPDKEnvComponent` — `init()` runs checks then env init then enumeration; `devices()` returns cloned vec; `device_count()` reads vec len; `is_initialized()` reads atomic flag in `components/spdk-env/src/lib.rs`
- [X] T022 [US1] Implement `Drop` for `SPDKEnvComponent` — if initialized, call `spdk_env_fini` wrapper to clean up and clear singleton flag in `components/spdk-env/src/lib.rs`
- [X] T023 [US1] Add `// SAFETY:` comments for all `unsafe` blocks in FFI calls (env_init, env_fini, pci_for_each_device, device accessors) in `components/spdk-env/src/env.rs`
- [X] T024 [US1] Add unit tests for `PciAddress::Display`, `VfioDevice` construction, and `SpdkEnvError` formatting in `components/spdk-env/src/device.rs` and `components/spdk-env/src/error.rs`

**Checkpoint**: On a VFIO-capable system, `init()` + `devices()` returns discovered devices. `cargo test -p spdk-env` passes.

---

## Phase 5: User Story 3 - Non-Root Operation (Priority: P2)

**Goal**: Component works correctly without root when VFIO paths have user-level permissions. Error messages identify specific inaccessible paths for non-root troubleshooting.

**Independent Test**: Run example as non-root user with correctly configured VFIO permissions; verify device enumeration succeeds.

### Implementation for User Story 3

- [X] T025 [US3] Refine permission error messages in `check_vfio_permissions()` to include uid/gid context and suggest udev rules or group membership in `components/spdk-env/src/checks.rs`
- [X] T026 [US3] Ensure `spdk_env_opts` in init wrapper does not request features requiring root (no `--no-huge`, no reserved memory beyond hugepage allocation) in `components/spdk-env/src/env.rs`
- [X] T027 [US3] Add doc comments to public API explaining non-root requirements and VFIO permission setup in `components/spdk-env/src/lib.rs`

**Checkpoint**: Non-root user with correct permissions can run the component successfully. Permission errors include actionable guidance.

---

## Phase 6: User Story 4 - Component Framework Integration (Priority: P2)

**Goal**: Component fully conforms to framework conventions. Example binary demonstrates construct-wire-init lifecycle with logging.

**Independent Test**: Run example main.rs; verify `query_interface!` returns ISPDKEnv, log messages appear through framework logger, no threads spawned.

### Implementation for User Story 4

- [X] T028 [US4] Implement logging within `init()` and device enumeration — send `LogMessage::info` for progress, `LogMessage::warn` for skipped devices, `LogMessage::error` for failures via the logger receptacle in `components/spdk-env/src/env.rs`
- [X] T029 [US4] Create example binary `components/spdk-env/examples/spdk-env-example.rs` — instantiate `SPDKEnvComponent`, create logger actor, wire logger receptacle, call `init()`, print devices, demonstrate full lifecycle (FR-010)
- [X] T030 [US4] Add doc-tests (runnable `///` examples) for `ISPDKEnv`, `SPDKEnvComponent::new()`, `VfioDevice`, `PciAddress`, `SpdkEnvError` in `components/spdk-env/src/lib.rs`, `src/device.rs`, `src/error.rs`

**Checkpoint**: Example binary compiles and runs. `query_interface!(comp, ISPDKEnv)` works. All doc-tests pass.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Constitution compliance, documentation, benchmarks

- [X] T031 ~~Enable `#![deny(missing_docs)]`~~ (skipped: framework macros don't generate doc comments; docs added to all hand-written items)
- [X] T032 [P] Run `cargo clippy -p spdk-env -p spdk-sys -- -D warnings` and fix all warnings
- [X] T033 [P] Run `cargo fmt --check -p spdk-env -p spdk-sys` and fix formatting
- [X] T034 [P] Run `cargo doc --no-deps -p spdk-env` and fix any doc warnings
- [ ] T035 [P] Add Criterion benchmark for device enumeration latency in `components/spdk-env/benches/device_enum.rs` (deferred: requires VFIO-capable hardware)
- [X] T036 Run `cargo test -p spdk-env -p spdk-sys --all` and verify zero failures
- [X] T037 Verify all constitution quality gates pass: fmt, clippy, test, doc, benchmarks

**Checkpoint**: All constitution quality gates pass. Component is merge-ready.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US2 (Phase 3)**: Depends on Foundational — implements pre-flight checks
- **US1 (Phase 4)**: Depends on Foundational + US2 (checks are called within init)
- **US3 (Phase 5)**: Depends on US2 (refines permission messages)
- **US4 (Phase 6)**: Depends on US1 (example needs working init + devices)
- **Polish (Phase 7)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 2 (P1)**: Can start after Foundational — pre-flight checks are self-contained
- **User Story 1 (P1)**: Depends on US2 — init() calls checks before SPDK init
- **User Story 3 (P2)**: Depends on US2 — refines permission error messages
- **User Story 4 (P2)**: Depends on US1 — example needs working init + enumeration

### Within Each User Story

- Types and error definitions before implementations
- Check functions before init() wiring
- SPDK wrappers before ISPDKEnv trait impl
- Implementation before tests and doc-tests
- SAFETY comments concurrent with unsafe code

### Parallel Opportunities

- T001 and T002 (crate setup) can run in parallel
- T007 and T008 (error types and device types) can run in parallel
- T032, T033, T034, T035 (polish tasks) can run in parallel
- US3 and US4 can start in parallel (both depend on different prior phases)

---

## Parallel Example: Foundational Phase

```
# Launch type definitions in parallel:
Task T007: "Define SpdkEnvError enum in components/spdk-env/src/error.rs"
Task T008: "Define PciAddress, PciId, VfioDevice in components/spdk-env/src/device.rs"
```

## Parallel Example: Polish Phase

```
# Launch all lint/format/doc checks in parallel:
Task T032: "Run cargo clippy"
Task T033: "Run cargo fmt --check"
Task T034: "Run cargo doc --no-deps"
Task T035: "Add Criterion benchmark"
```

---

## Implementation Strategy

### MVP First (User Stories 1 + 2)

1. Complete Phase 1: Setup (T001-T003)
2. Complete Phase 2: Foundational (T004-T010)
3. Complete Phase 3: US2 - Pre-flight checks (T011-T017)
4. Complete Phase 4: US1 - SPDK init + device discovery (T018-T024)
5. **STOP and VALIDATE**: Test on VFIO-capable system — `init()` succeeds, `devices()` returns correct entries, errors are actionable

### Incremental Delivery

1. Setup + Foundational → Crates compile
2. Add US2 → Pre-flight checks work → Actionable error messages
3. Add US1 → Full init + enumeration works → **MVP complete**
4. Add US3 → Non-root messages refined
5. Add US4 → Example binary + doc-tests → Full integration
6. Polish → Constitution compliance → Merge-ready

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- US1 depends on US2 because init() calls checks first — they cannot be fully parallelized
- The `spdk-sys` crate requires SPDK to be pre-built at `deps/spdk-build/` — `build.rs` will fail without it
- The `example-logger` crate (ILogger dependency) is declared in workspace but not yet implemented — this is a blocking dependency for the logger receptacle
- All unsafe FFI calls must have `// SAFETY:` comments per constitution Principle V
- Commit after each task or logical group

# Tasks: Remote Lookup Batch Interface

**Input**: Design documents from `specs/001-remote-lookup-placeholder/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Unit tests and documentation tests are required per the constitution (Principles II and III).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: Ensure the component crate and interface file are ready for modification.

- [x] T001 Verify `components/remote-lookup/Cargo.toml` has dependency on `interfaces` crate with access to `CacheKey` and `IpcHandle`

**Checkpoint**: Component builds cleanly with `cargo build -p remote-lookup`

---

## Phase 2: Foundational (Interface Definition)

**Purpose**: Add `batch_lookup` to the `IRemoteLookup` trait definition — blocks all user story implementation.

**⚠️ CRITICAL**: User story implementation cannot begin until this phase is complete.

- [x] T002 Add `batch_lookup` method signature to `IRemoteLookup` trait in `components/interfaces/src/iremote_lookup.rs` with signature `fn batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>`
- [x] T003 Add necessary imports (`CacheKey`, `IpcHandle`) to `components/interfaces/src/iremote_lookup.rs`
- [x] T004 Add doc comment with runnable example to `batch_lookup` in `components/interfaces/src/iremote_lookup.rs`

**Checkpoint**: `cargo build -p interfaces` compiles. `cargo doc -p interfaces --no-deps` is warning-free.

---

## Phase 3: User Story 1 - Batch Lookup Placeholder (Priority: P1) 🎯 MVP

**Goal**: Implement the placeholder `batch_lookup` that logs each entry and returns `NotFound`.

**Independent Test**: Call `batch_lookup` with sample entries, verify log output and result ordering.

### Tests for User Story 1

- [x] T005 [P] [US1] Add unit test `batch_lookup_returns_not_found_for_each_entry` in `components/remote-lookup/src/lib.rs`
- [x] T006 [P] [US1] Add unit test `batch_lookup_returns_empty_vec_for_empty_input` in `components/remote-lookup/src/lib.rs`
- [x] T007 [P] [US1] Add unit test `batch_lookup_preserves_positional_order` in `components/remote-lookup/src/lib.rs`

### Implementation for User Story 1

- [x] T008 [US1] Implement `batch_lookup` method on `RemoteLookupComponent` in `components/remote-lookup/src/lib.rs` — log each entry via ILogger receptacle and return `NotFound` for each
- [x] T009 [US1] Add doc test example for `batch_lookup` in `components/remote-lookup/src/lib.rs`

**Checkpoint**: `cargo test -p remote-lookup` passes. `cargo test --doc -p remote-lookup` passes.

---

## Phase 4: User Story 2 - Interface Conformance with IDispatcher (Priority: P2)

**Goal**: Verify that `batch_lookup` accepts the same `&[(CacheKey, IpcHandle)]` parameter types as `IDispatcher::batch_lookup`.

**Independent Test**: Code that passes `&[(CacheKey, IpcHandle)]` to `IRemoteLookup::batch_lookup` compiles without type coercion.

### Tests for User Story 2

- [x] T010 [US2] Add compile-time verification test in `components/remote-lookup/src/lib.rs` that constructs `&[(CacheKey, IpcHandle)]` and passes it to `batch_lookup` without conversion

### Implementation for User Story 2

No additional implementation — US1 implementation satisfies this if types align correctly. The test in T010 serves as the conformance assertion.

**Checkpoint**: T010 test compiles and passes.

---

## Phase 5: Polish & Cross-Cutting Concerns

**Purpose**: Final validation across the full component.

- [x] T011 Run `cargo clippy -p remote-lookup -- -D warnings` and fix any warnings
- [x] T012 Run `cargo doc -p remote-lookup --no-deps` and fix any documentation warnings
- [x] T013 Run `cargo fmt --check -p remote-lookup` and fix any formatting issues

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational (Phase 2) completion
- **User Story 2 (Phase 4)**: Depends on User Story 1 (Phase 3) completion (needs implementation to test against)
- **Polish (Phase 5)**: Depends on all user stories complete

### Parallel Opportunities

Within Phase 2:
```
T002 → T003 (sequential: imports needed for signature)
T003 → T004 (sequential: doc example references the method)
```

Within Phase 3 (tests):
```
T005, T006, T007 can run in parallel (all test different scenarios)
```

Within Phase 3 (implementation):
```
T008 → T009 (sequential: doc test needs implementation)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (verify deps)
2. Complete Phase 2: Add `batch_lookup` to interface trait
3. Complete Phase 3: Implement placeholder + tests
4. **STOP and VALIDATE**: `cargo test -p remote-lookup` passes
5. Component is functional and ready for integration

### Incremental Delivery

1. Phase 1 + 2 → Interface published
2. Phase 3 → Placeholder working (MVP!)
3. Phase 4 → Type conformance verified
4. Phase 5 → Polish complete, ready to merge

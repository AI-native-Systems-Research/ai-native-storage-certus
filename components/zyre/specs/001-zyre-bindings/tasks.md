# Tasks: Zyre Rust Bindings

**Input**: Design documents from `/specs/001-zyre-bindings/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — the constitution mandates unit tests, doc tests, and integration tests for all public APIs.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Build system, dependency scripts, and FFI foundation

- [x] T001 Create `deps/install_zyre_deps.sh` script to install system prerequisites (cmake, pkg-config, libtool, libclang) in `deps/install_zyre_deps.sh`
- [x] T002 Create `deps/build_zyre.sh` script to clone and build libzmq v4.3.5, czmq v4.2.1, zyre v2.0.1 into `deps/zyre-build/` in `deps/build_zyre.sh`
- [x] T003 Create `build.rs` with bindgen invocation against `deps/zyre-build/include/` headers and link configuration in `components/zyre/build.rs`
- [x] T004 [P] Create `src/ffi.rs` with `include!()` of generated bindings and internal helper functions in `components/zyre/src/ffi.rs`
- [x] T005 [P] Update `components/zyre/Cargo.toml` to add bindgen build-dependency and libc dependency in `components/zyre/Cargo.toml`
- [x] T006 Remove `components/zyre` from `default-members` in workspace `Cargo.toml` (requires pre-built C deps) in `Cargo.toml`
- [x] T007 [P] Add `deps/zyre/` and `deps/zyre-build/` to `.gitignore` in `.gitignore`

**Checkpoint**: `cargo build -p zyre` compiles (with C deps pre-built) and FFI bindings are generated.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and error handling that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

> **Note (2026-07-09)**: T008–T013 were originally written against an earlier
> design (per-type source files `error.rs`/`peer.rs`/`event.rs`/`builder.rs`, a
> `NodeConfigBuilder`, and a `ping()`-only `IZyre`). That design was superseded
> by the factory refactor (commit `b45418d`): the value types and the
> `IZyre`/`IZyreNode` traits live together in `components/interfaces/src/izyre.rs`
> (so `IZyre::create_node` can name them without a crate cycle), `NodeConfig`
> uses public fields + `Default` (no builder), and `ZyreNode` is crate-private.
> The tasks below reflect the design as shipped.

- [x] T008 Implement `ZyreError` enum with all variants (CreateFailed, StartFailed, NotStarted, InvalidConfig, SendFailed, RecvFailed) and Display/Error impls in `components/interfaces/src/izyre.rs`
- [x] T009 Implement `PeerId` newtype with Clone, Debug, Display, PartialEq, Eq, Hash derives in `components/interfaces/src/izyre.rs`
- [x] T010 [P] Implement `ZyreEvent` enum with all 9 variants (Enter, Exit, Evasive, Silent, Join, Leave, Whisper, Shout, Stop) plus `peer()`/`peer_name()`/`group()` accessors in `components/interfaces/src/izyre.rs`
- [x] T011 [P] Implement `NodeConfig` (public fields + `Default`, `#[non_exhaustive]`) and `GossipConfig` with validation in `components/interfaces/src/izyre.rs`
- [x] T012 Declare the `zyre` crate modules (`ffi`, `node`) and re-export the public types from `interfaces` in `components/zyre/src/lib.rs`
- [x] T013 Define `IZyre` as a factory (`ping()` + `create_node(config) -> Box<dyn IZyreNode>`) and the `IZyreNode` handle trait in `components/interfaces/src/izyre.rs`; the concrete `ZyreNode` stays crate-private in the `zyre` crate

**Checkpoint**: All foundational types compile. `cargo check -p zyre` passes (even without node implementation).

---

## Phase 3: User Story 1 - Discover and Communicate with Peers (Priority: P1) 🎯 MVP

**Goal**: Two Zyre nodes discover each other on a LAN and exchange messages via shout/whisper.

**Independent Test**: Start two nodes in separate threads, join a group, shout a message, verify receipt.

### Tests for User Story 1

- [x] T014 [P] [US1] Unit tests for `ZyreNode::new()`, `start()`, `stop()` lifecycle (config validation tests) in `components/zyre/src/node.rs` (inline tests module)
- [x] T015 [P] [US1] Unit tests for `ZyreEvent` accessors (peer, peer_name, group) in `components/interfaces/src/izyre.rs` (inline tests module)
- [x] T016 [US1] Integration test: two-node discovery and shout round-trip on localhost in `components/zyre/tests/integration.rs`

### Implementation for User Story 1

- [x] T017 [US1] Implement `ZyreNode` struct with `new(config)`, wrapping `zyre_new()` + config application in `components/zyre/src/node.rs`
- [x] T018 [US1] Implement `ZyreNode::start()` and `ZyreNode::stop()` wrapping `zyre_start()`/`zyre_stop()` in `components/zyre/src/node.rs`
- [x] T019 [US1] Implement `ZyreNode::join(group)` and `ZyreNode::leave(group)` in `components/zyre/src/node.rs`
- [x] T020 [US1] Implement `ZyreNode::shout(group, &[u8])` and `ZyreNode::whisper(peer, &[u8])` for single-frame sends in `components/zyre/src/node.rs`
- [x] T021 [US1] Implement `ZyreNode::recv() -> Result<ZyreEvent, ZyreError>` blocking receive with event parsing in `components/zyre/src/node.rs`
- [x] T022 [US1] Implement `ZyreNode::try_recv() -> Result<Option<ZyreEvent>, ZyreError>` non-blocking poll in `components/zyre/src/node.rs`
- [x] T023 [US1] Implement `Drop` for `ZyreNode` calling `zyre_stop()` + `zyre_destroy()` in `components/zyre/src/node.rs`
- [x] T024 [US1] Implement `Send` marker (unsafe impl Send for ZyreNode) with SAFETY comment in `components/zyre/src/node.rs`
- [x] T025 [US1] Implement event parsing: convert `zyre_event_t` accessors into `ZyreEvent` enum variants in `components/zyre/src/node.rs` (parse_event function)
- [x] T026 [US1] Implement `ZyreNode::uuid()` and `ZyreNode::name()` accessors in `components/zyre/src/node.rs`

**Checkpoint**: Two nodes can discover each other and exchange messages on localhost. `cargo test -p zyre` passes.

---

## Phase 4: User Story 2 - Idiomatic Rust API (Priority: P1)

**Goal**: The API follows Rust conventions: public-field config structs, Result types, RAII, doc comments with examples.

**Independent Test**: Compile user code without `unsafe`, verify nodes are cleaned up on drop, verify `NodeConfig` validation rejects bad config at `create_node`.

### Tests for User Story 2

- [x] T027 [P] [US2] Unit tests for `NodeConfig::validate` (invalid timeouts, empty name, gossip invariants) in `components/interfaces/src/izyre.rs` (inline tests module)
- [x] T028 [P] [US2] Doc tests on all public types/methods demonstrating correct usage in `components/zyre/src/lib.rs`, `components/zyre/src/node.rs`, and `components/interfaces/src/izyre.rs`
- [x] T029 [US2] Compile-time test: verify no `unsafe` in public API surface and assert `ZyreNode: Send + !Sync` in `components/zyre/tests/api_safety.rs`

### Implementation for User Story 2

- [x] T030 [P] [US2] Add doc comments with `///` examples to `ZyreNode` (all public methods) in `components/zyre/src/node.rs`
- [x] T031 [P] [US2] Add doc comments with `///` examples to `ZyreEvent` and all variants in `components/interfaces/src/izyre.rs`
- [x] T032 [P] [US2] Add doc comments with `///` examples to `NodeConfig` and `GossipConfig` (public fields + constructors) in `components/interfaces/src/izyre.rs`
- [x] T033 [P] [US2] Add doc comments with `///` examples to `ZyreError` and `PeerId` in `components/interfaces/src/izyre.rs`
- [x] T034 [US2] Add crate-level documentation (`//!`) with quickstart example in `components/zyre/src/lib.rs`
- [ ] T035 [US2] Verify `cargo doc --no-deps -p zyre` completes with zero warnings (requires Linux with C deps)
- [ ] T036 [US2] Verify `cargo clippy -p zyre -- -D warnings` passes clean (requires Linux with C deps)

**Checkpoint**: `cargo test --doc -p zyre` passes. `cargo doc --no-deps -p zyre` is warning-free. All public APIs have examples.

---

## Phase 5: User Story 3 - Build Integration with Sub-Repo (Priority: P2)

**Goal**: A developer can build zyre from a clean checkout by running two scripts.

**Independent Test**: Run `deps/build_zyre.sh` on a clean machine, then `cargo build -p zyre` succeeds.

### Tests for User Story 3

- [x] T037 [US3] Verify `deps/build_zyre.sh` is idempotent (re-running does not fail or re-clone) — manual test documented in `specs/001-zyre-bindings/quickstart.md`
- [x] T038 [US3] Verify build script pins exact versions (libzmq v4.3.5, czmq v4.2.1, zyre v2.0.1) — inspection test in CI script

### Implementation for User Story 3

- [x] T039 [US3] Finalize `deps/install_zyre_deps.sh` with RHEL/Fedora package list (cmake, gcc, make, pkg-config, libtool, libclang-devel) in `deps/install_zyre_deps.sh`
- [x] T040 [US3] Finalize `deps/build_zyre.sh` with version pinning, idempotent clone, cmake build, and local install prefix in `deps/build_zyre.sh`
- [x] T041 [US3] Ensure `build.rs` uses `ZYRE_BUILD_DIR` env var with fallback to `deps/zyre-build/` in `components/zyre/build.rs`
- [ ] T042 [US3] Verify end-to-end: clean `deps/zyre-build/`, run `deps/build_zyre.sh`, then `cargo build -p zyre` succeeds (requires Linux)

**Checkpoint**: Clean-checkout build works. No system-wide library installation required.

---

## Phase 6: User Story 4 - Gossip-Based Discovery (Priority: P3)

**Goal**: Nodes can use gossip discovery instead of UDP beaconing for environments without broadcast.

**Independent Test**: Start nodes with gossip endpoints, verify discovery without UDP beaconing.

### Tests for User Story 4

- [x] T043 [P] [US4] Unit test for `GossipConfig` validation (at least one of bind/connect required) in `components/interfaces/src/izyre.rs`
- [x] T044 [US4] Integration test: two nodes discover each other via gossip (no UDP beacon) in `components/zyre/tests/integration.rs`

### Implementation for User Story 4

- [x] T045 [US4] Implement gossip configuration application in `ZyreNode::new()` — call `zyre_gossip_bind()` / `zyre_gossip_connect()` and `zyre_set_endpoint()` in `components/zyre/src/node.rs`
- [x] T046 [US4] Add doc examples for gossip configuration (`GossipConfig`) in `components/interfaces/src/izyre.rs`

**Checkpoint**: Gossip discovery works. Both beacon and gossip paths tested.

---

## Phase 7: User Story 1+2 Component Integration (Priority: P1)

**Goal**: Wire ZyreNode into the Certus component framework via IZyre factory interface.

**Independent Test**: `query_interface!(comp, IZyre)` returns the factory; calling `create_node()` returns a working node.

### Tests for Component Integration

- [x] T047 [P] [US1] Unit test for `ZyreComponent` implementing `IZyre::ping()` in `components/zyre/src/lib.rs` (inline tests module)

### Implementation for Component Integration

- [x] T048 [US1] Implement `IZyre` for `ZyreComponent` — `ping()` returns "pong" and `create_node(config)` validates the config and returns `Box::new(ZyreNode::new(config)?)` in `components/zyre/src/lib.rs`
- [x] T049 [US1] Export `IZyre`, `IZyreNode`, and the value types (`NodeConfig`, `GossipConfig`, `ZyreEvent`, `PeerId`, `ZyreError`) from `components/interfaces/src/lib.rs`; the `zyre` crate re-exports them

**Checkpoint**: Component can be instantiated via the framework and used to create functional nodes.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Final quality gates and documentation

- [~] T050 [P] ~~Add `shout_multi(group, &[&[u8]])` and `whisper_multi(peer, &[&[u8]])` multi-frame variants~~ **Removed 2026-07-09**: a single frame is bounded only by memory, so the single-frame `shout`/`whisper` API covers arbitrarily large payloads. The `_multi` send methods were dropped (receive only ever surfaced the first frame). See the Session 2026-07-09 clarification in `spec.md`.
- [x] T051 [P] Implement `ZyreNode::peers()`, `ZyreNode::peers_by_group()`, `ZyreNode::own_groups()`, `ZyreNode::peer_groups()` introspection in `components/zyre/src/node.rs`
- [x] T052 [P] Implement `ZyreNode::peer_address()` and `ZyreNode::peer_header_value()` in `components/zyre/src/node.rs`
- [x] T053 [P] Add unit tests for peer introspection methods in `components/zyre/src/node.rs` (validation-level tests; full introspection requires running C library)
- [x] T054 Run `cargo fmt --check -p zyre` and fix any formatting issues
- [ ] T055 Run full CI gate: `cargo fmt --check && cargo clippy -p zyre -- -D warnings && cargo test -p zyre && cargo doc --no-deps -p zyre` (requires Linux with C deps)
- [ ] T056 Validate quickstart.md instructions end-to-end in `specs/001-zyre-bindings/quickstart.md` (requires Linux)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 completion (build.rs and ffi.rs must exist)
- **User Story 1 (Phase 3)**: Depends on Phase 2 — BLOCKS most other stories
- **User Story 2 (Phase 4)**: Depends on Phase 3 (needs implemented APIs to document)
- **User Story 3 (Phase 5)**: Can start after Phase 1 (independent — just build scripts)
- **User Story 4 (Phase 6)**: Depends on Phase 3 (needs working node to add gossip)
- **Component Integration (Phase 7)**: Depends on Phase 3 (needs ZyreNode)
- **Polish (Phase 8)**: Depends on Phases 3, 4, 6, 7

### User Story Dependencies

- **US1 (Peer Discovery)**: After Foundational — no other story dependencies
- **US2 (Idiomatic API)**: After US1 — needs implemented methods to add docs
- **US3 (Build Integration)**: After Setup — fully independent of US1/US2
- **US4 (Gossip Discovery)**: After US1 — extends node configuration

### Within Each User Story

- Tests written alongside implementation (constitution mandates TDD preferred)
- Types/structs before methods
- Core operations before convenience methods
- Implementation before documentation polish

### Parallel Opportunities

- T004 + T005 + T007 (Phase 1: independent files)
- T010 + T011 (Phase 2: the `ZyreEvent` and `NodeConfig`/`GossipConfig` types)
- T014 + T015 (Phase 3 tests: different files)
- T027 + T028 + T029 (Phase 4 tests: different files)
- T030 + T031 + T032 + T033 (Phase 4: doc comments on different files)
- T043 + T044 (Phase 6 tests)
- T050 + T051 + T052 + T053 (Phase 8: independent additions)
- **US3 can proceed in parallel with US1** (build scripts are independent of Rust implementation)

---

## Parallel Example: User Story 1

```bash
# After Phase 2 completes, launch tests and implementation in parallel:
Task T014: "Unit tests for ZyreNode lifecycle in zyre/src/node.rs"
Task T015: "Unit tests for ZyreEvent accessors in interfaces/src/izyre.rs"

# Then implement sequentially (same file dependencies):
Task T017 → T018 → T019 → T020 → T021 → T022 → T023 → T024 (all in zyre/src/node.rs)
Task T025 (event parsing, in zyre/src/node.rs)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (build scripts + FFI)
2. Complete Phase 2: Foundational (types + errors)
3. Complete Phase 3: User Story 1 (node lifecycle + messaging)
4. **STOP and VALIDATE**: Two nodes discover and communicate on localhost
5. Merge as working MVP

### Incremental Delivery

1. Setup + Foundational → Build compiles with FFI bindings
2. Add US1 → Two-node messaging works → MVP!
3. Add US2 → Full doc coverage, clippy clean → Production-ready API
4. Add US3 → Clean-checkout build works → Contributor-friendly
5. Add US4 → Gossip mode → Broader deployment support
6. Polish → introspection, final CI gate

### Parallel Team Strategy

With multiple developers:

1. All: Complete Setup + Foundational together
2. Once Phase 2 done:
   - Developer A: US1 (node implementation) + US4 (gossip extends node)
   - Developer B: US3 (build scripts — independent)
3. After US1 merges:
   - Developer A: US2 (documentation) + Phase 7 (component integration)
   - Developer B: Phase 8 (polish)

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Constitution requires: doc tests, unit tests, integration tests, clippy clean, cargo doc clean
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently

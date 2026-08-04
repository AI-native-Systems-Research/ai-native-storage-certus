---

description: "Task list for Session-Lineage Eviction Policy"
---

# Tasks: Session-Lineage Eviction Policy

**Input**: Design documents from `specs/001-session-list-eviction/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/ieviction_policy_track.md, quickstart.md

**Tests**: INCLUDED. The project constitution mandates unit + Rust doc tests + Criterion perf tests for every public API, and SC-006 requires a lineage-invariant property test. Test tasks are therefore first-class here.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)
- Exact file paths are given in each task.

## Path Conventions

Single Rust library crate (Certus `define_component!`). Component code lives under
`components/eviction-policy-session-lists/`; the shared interface change lives under
`components/interfaces/`; mechanical caller/impl updates touch `components/eviction-policy-lru/`,
`components/dispatch-map/`, and `components/memory-tier/`. Paths below are repository-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Crate scaffolding for the new module, benches, and tests.

- [X] T001 [P] Add `criterion` dev-dependency and a `[[bench]]` entry (`name = "session_list_benchmark"`, `harness = false`) to `components/eviction-policy-session-lists/Cargo.toml`
- [X] T002 Create file/module skeleton: `components/eviction-policy-session-lists/src/session_list.rs` (empty `pub(crate)` module), `components/eviction-policy-session-lists/benches/session_list_benchmark.rs`, `components/eviction-policy-session-lists/tests/lineage_properties.rs`; declare `mod session_list;` in `components/eviction-policy-session-lists/src/lib.rs` and confirm `cargo build -p eviction-policy-session-lists` still compiles

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The shared interface change, the mechanical workspace updates that keep every crate compiling, and the core arena data structure + registration primitive that all three user stories build on.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

### Interface change (`components/interfaces`) — the single breaking change

- [X] T003 Add `pub type SessionId = u64;` and `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)] pub struct BlockSemantics { pub session_id: SessionId }` (with doc comments per contract) to `components/interfaces/src/ieviction_policy.rs`
- [X] T004 Extend the `track` signature inside `define_interface! { IEvictionPolicy { … } }` to `fn track(&self, pool: PoolId, key: CacheKey, semantics: BlockSemantics) -> Result<EvictionHandle, EvictionPolicyError>;` in `components/interfaces/src/ieviction_policy.rs`
- [X] T005 Re-export `SessionId` and `BlockSemantics` (`pub use ieviction_policy::{SessionId, BlockSemantics};`) from `components/interfaces/src/lib.rs`

### Mechanical, behavior-preserving updates to the other implementor + callers

- [X] T006 [P] `components/eviction-policy-lru/src/lib.rs`: add ignored `_semantics: BlockSemantics` param to the `track` impl (line 53); update the in-crate test call sites (lines 192–319) and the `components/eviction-policy-lru/README.md` examples (lines 52–53) to pass `BlockSemantics::default()`
- [X] T007 [P] `components/dispatch-map/src/lib.rs`: pass `BlockSemantics::default()` at the 3 `ep.track(...)` sites (lines 91, 392, 572)
- [X] T008 [P] `components/memory-tier/src/lib.rs`: pass `BlockSemantics::default()` at the `ep.track(...)` site (line 365)
- [X] T009 Verify the default-member workspace builds and pre-existing tests still pass after the signature change: `cargo build` and `cargo test -p eviction-policy-lru -p dispatch-map -p memory-tier -- --test-threads 1`

### Core data structure + registration primitive (shared by US1/US2/US3)

- [X] T010 Define `Node { key: CacheKey, session: SessionId, parent: Option<u32>, child: Option<u32>, stamp: u64, active: bool }` in `components/eviction-policy-session-lists/src/session_list.rs`
- [X] T011 Define per-pool `Pool { nodes: Vec<Node>, free: Vec<u32>, by_key: HashMap<CacheKey,u32>, sessions: HashMap<SessionId,u32>, leaves: BTreeSet<(u64,u32)>, clock: u64, len: usize }` in `components/eviction-policy-session-lists/src/session_list.rs`; replace the placeholder `Pool`/`EvictionState` in `components/eviction-policy-session-lists/src/lib.rs` and update the `interfaces` import to add `BlockSemantics`, `SessionId`
- [X] T012 Implement arena alloc/free helpers (free-list reuse; monotonic `clock` bump) and the internal `register` primitive for a **new** key — allocate a node, link it as child of the session's current leaf (or head+leaf if the session is new), update `sessions`/`leaves`/`by_key` — in `components/eviction-policy-session-lists/src/session_list.rs`
- [X] T013 Wire the new-signature `IEvictionPolicy::track` in `components/eviction-policy-session-lists/src/lib.rs` to delegate to the `register` primitive for a fresh key and return its `EvictionHandle`; implement `len` (FR-013) and `clear_pool` (FR-014) over `session_list::Pool`; invalid pool → `Err(InvalidPool(pool))` (FR-015)

**Checkpoint**: Workspace compiles; pools can be created, blocks registered into session chains, counted, and cleared through `IEvictionPolicy`. User-story work can begin.

---

## Phase 3: User Story 1 - Select an eviction victim that preserves lineage (Priority: P1) 🎯 MVP

**Goal**: Given populated multi-session chains, return the oldest-accessed *leaf* across all sessions in a pool, never an interior/head block that still has a tracked descendant; also list up to N candidates without removing them.

**Independent Test**: Register several sessions with multi-block chains (via the foundational registration path) with differing access stamps, request a victim, and confirm it is the oldest-accessed leaf and never a block with a tracked child; request N candidates and confirm order + that none are removed.

### Tests for User Story 1 (write first; expect FAIL) ⚠️

- [X] T014 [P] [US1] Unit tests in `components/eviction-policy-session-lists/src/session_list.rs` (`#[cfg(test)]`): oldest leaf across sessions selected and stops being tracked (spec US1 §1); chain A→B→C evicts C then B becomes eligible (US1 §2, FR-009); empty domain → `None` (US1 §3); `get_eviction_candidates(n)` returns up to n leaves in eviction order and removes none (US1 §4, FR-010); never selects a node with a tracked child (SC-004); deterministic tie-break (FR-012)

### Implementation for User Story 1

- [X] T015 [US1] Implement `identify_next_to_evict` (FR-006/007/008/012) in `components/eviction-policy-session-lists/src/session_list.rs` and wire it in `components/eviction-policy-session-lists/src/lib.rs`: pop the smallest `(stamp,idx)` from `leaves`, splice the node out, promote its parent to leaf if the parent becomes childless (FR-009), return the evicted `CacheKey` (no recency refresh — FR-006); `None` when empty
- [X] T016 [US1] Implement `get_eviction_candidates(pool, n)` (FR-010) in `components/eviction-policy-session-lists/src/session_list.rs` + `components/eviction-policy-session-lists/src/lib.rs`: return up to `n` oldest-leaf `CacheKey`s in eviction order, removing none
- [X] T017 [US1] Add a Rust doc test (constitution III) on the component / `identify_next_to_evict` in `components/eviction-policy-session-lists/src/lib.rs` demonstrating lineage-preserving victim selection

**Checkpoint**: User Story 1 is fully functional and independently testable — the MVP eviction behavior works end-to-end through `IEvictionPolicy`.

---

## Phase 4: User Story 2 - Register a block into its session's lineage (Priority: P2)

**Goal**: Registration builds correct per-session lineage: first block is head+leaf; a later block becomes the child (new leaf) of the session's current leaf; distinct sessions are independent chains; re-registering a tracked key is an idempotent recency refresh; blocks can be removed with the chain re-spliced.

**Independent Test**: Register A then B then C for one session and verify chain A(head)→B→C(leaf); register under a second session id and verify independence; re-register a tracked key and verify no new node and the same handle; remove an interior block and verify the chain stays consistent.

### Tests for User Story 2 (write first; expect FAIL) ⚠️

- [X] T018 [P] [US2] Unit tests in `components/eviction-policy-session-lists/src/session_list.rs` (`#[cfg(test)]`): first block is head+leaf (FR-001); second block gets first as parent and becomes leaf (FR-002); distinct `session_id`s form independent chains that never appear in one another's lineage (FR-003); re-registering an already-tracked key refreshes recency, returns the existing handle, and creates no new node / no lineage change (FR-017); `remove` of an interior block re-links child→parent (FR-011); invalid handle → error (FR-015)

### Implementation for User Story 2

- [X] T019 [US2] Implement the idempotent re-registration path of `track` (FR-017) in `components/eviction-policy-session-lists/src/session_list.rs` + `components/eviction-policy-session-lists/src/lib.rs`: on an already-tracked key, refresh the node's stamp and return its existing handle without allocating a node or altering lineage
- [X] T020 [US2] Implement `remove(handle)` (FR-011, FR-009, FR-015) in `components/eviction-policy-session-lists/src/session_list.rs` + `components/eviction-policy-session-lists/src/lib.rs`: splice the node (relink `child.parent → parent`, `parent.child → child`), update `leaves`/`sessions`/`by_key`, free the slot; if the removed node was a leaf, promote its parent; invalid/removed handle → `Err(InvalidHandle)`
- [X] T021 [US2] Add a Rust doc test on `track` in `components/eviction-policy-session-lists/src/lib.rs` showing chain construction A→B→C and two independent sessions

**Checkpoint**: Registration, idempotent refresh, and removal all maintain lineage; User Stories 1 and 2 work independently.

---

## Phase 5: User Story 3 - Refresh recency on access (Priority: P3)

**Goal**: Accessing a block refreshes its most-recent-access stamp (single or batched); the block being evicted is not refreshed by the eviction itself.

**Independent Test**: Register two equal-recency leaves, touch one, request a victim, and confirm the untouched (older) leaf is chosen; apply a batch touch and confirm all listed blocks are refreshed; confirm eviction does not refresh the evicted block.

### Tests for User Story 3 (write first; expect FAIL) ⚠️

- [X] T022 [P] [US3] Unit tests in `components/eviction-policy-session-lists/src/session_list.rs` (`#[cfg(test)]`): touching one of two equal-recency leaves makes the other the victim (US3 §1); a batch touch refreshes every listed block (US3 §2, FR-005); eviction does not refresh the evicted block (US3 §3, FR-006); `touch`/`batch_touch` on an invalid handle → error (FR-015)

### Implementation for User Story 3

- [X] T023 [US3] Implement `touch(handle)` (FR-004) in `components/eviction-policy-session-lists/src/session_list.rs` + `components/eviction-policy-session-lists/src/lib.rs`: bump `clock`, set the node's `stamp`, and if the node is a leaf remove+reinsert its `(stamp,idx)` in `leaves`; invalid handle → `Err(InvalidHandle)`
- [X] T024 [US3] Implement `batch_touch(handles)` (FR-005) in `components/eviction-policy-session-lists/src/lib.rs`: group consecutive handles by pool to amortize lock acquisition (as `eviction-policy-lru` does) and apply `touch` to each
- [X] T025 [US3] Add a Rust doc test on `touch`/`batch_touch` in `components/eviction-policy-session-lists/src/lib.rs`

**Checkpoint**: All three user stories are independently functional.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Invariant/property coverage, performance validation, docs, and quality gates.

- [X] T026 [P] Property/invariant test in `components/eviction-policy-session-lists/tests/lineage_properties.rs` (SC-006): random sequences of track/touch/evict/remove preserve data-model invariants 1–6 (single linear chain, leaf-set exactness, session→leaf consistency, no orphans, key uniqueness, length agreement)
- [X] T027 [P] Criterion benches in `components/eviction-policy-session-lists/benches/session_list_benchmark.rs` (SC-002/003/005): `track`, `touch`, `batch_touch`, and `identify_next_to_evict` measured at scale (≥1M blocks; victim selection scaling with active sessions, not total blocks)
- [X] T028 [P] [US1] Extend the component integration test in `components/eviction-policy-session-lists/src/lib.rs` (`#[cfg(test)]`): `query_interface!` → full lifecycle (create_pool → track → touch → identify_next_to_evict → remove → clear_pool) across ≥2 pools through `IEvictionPolicy`
- [X] T029 Update the module docs in `components/eviction-policy-session-lists/src/lib.rs` (remove the "todo!() stubs / bootstrapped skeleton" wording) and ensure `cargo doc --no-deps -p eviction-policy-session-lists` is warning-free
- [X] T030 Run all quality gates: `cargo fmt --check`; `cargo clippy -p eviction-policy-session-lists -- -D warnings`; `cargo test -p eviction-policy-session-lists -- --test-threads 1`; `cargo bench -p eviction-policy-session-lists`; and walk the `quickstart.md` usage steps

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no dependencies — start immediately.
- **Foundational (Phase 2)**: depends on Setup. **Blocks all user stories.** T003→T004→T005 are ordered (same file / dependent symbols); T006/T007/T008 are parallel across crates and require T003–T005; T009 gates on T006–T008; T010→T011→T012→T013 are ordered (same files, dependent types).
- **User Stories (Phase 3–5)**: all depend on Foundational. They share `src/lib.rs` and `src/session_list.rs`, so run them in priority order (US1 → US2 → US3) rather than concurrently.
- **Polish (Phase 6)**: depends on all targeted user stories.

### User Story Dependencies

- **US1 (P1)**: depends only on Foundational (uses the T012 registration primitive + T013 `track` wiring to populate pools). Delivers the MVP.
- **US2 (P2)**: depends on Foundational; extends the same `track` path (idempotency) and adds `remove`. Independently testable.
- **US3 (P3)**: depends on Foundational; adds recency refresh. Independently testable.

### Within Each User Story

- Write the test task first and confirm it FAILS before implementing.
- `session_list.rs` mechanism before `lib.rs` wiring before doc tests.

### Parallel Opportunities

- Setup: T001 [P].
- Foundational: T006/T007/T008 [P] (three different crates) after the interface change lands.
- Polish: T026/T027/T028 [P] (three different files).
- Note: within a single story the test-writing task is marked [P] because it is a distinct file section, but the story's implementation tasks touch the shared `lib.rs`/`session_list.rs` and are sequential.

---

## Parallel Example: Foundational mechanical updates

```bash
# After T003–T005 (interface change) land, update the three downstream crates together:
Task: "eviction-policy-lru: add ignored _semantics param + pass BlockSemantics::default() in tests"
Task: "dispatch-map: pass BlockSemantics::default() at 3 track sites"
Task: "memory-tier: pass BlockSemantics::default() at 1 track site"
```

---

## Implementation Strategy

### MVP First (User Story 1)

1. Phase 1: Setup.
2. Phase 2: Foundational (interface change + mechanical updates + arena + registration primitive) — CRITICAL, blocks everything.
3. Phase 3: User Story 1 (eviction victim selection).
4. **STOP and VALIDATE**: register chains and confirm lineage-preserving victim selection through `IEvictionPolicy`.

### Incremental Delivery

1. Setup + Foundational → workspace compiles, pools populatable.
2. US1 → oldest-leaf eviction (MVP) → validate.
3. US2 → idempotent registration + removal/splicing → validate.
4. US3 → recency refresh (single + batch) → validate.
5. Polish → property test (SC-006), Criterion benches (SC-002/003/005), docs, quality gates.

---

## Notes

- [P] = different files, no dependency on incomplete tasks.
- Every public `IEvictionPolicy` method ends up with a unit test, a doc test, and (for hot paths) a Criterion bench, per the constitution.
- The only cross-crate blast radius is the mechanical `track` signature update (interface + `eviction-policy-lru` impl + `dispatch-map` ×3 + `memory-tier` ×1); all four call sites are production and pass `BlockSemantics::default()`.
- Commit after each task or logical group. Keep all work on `feat/component-eviction-policy-session-lists`.

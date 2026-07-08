# Tasks: Memory Tier DRAM Cache Pool

**Input**: Design documents from `/specs/001-memory-tier/`
**Prerequisites**: plan.md (required), spec.md (required)
**Context**: Backfilled from existing implementation. Tasks focus on review, testing gaps, and hardening.

---

## Phase 1: Review Backfilled Spec

**Purpose**: Validate that the auto-generated specification accurately reflects intended behavior and is complete.

- [ ] T001 Review generated user stories for accuracy against actual system usage patterns
- [ ] T002 Verify functional requirements FR-001 through FR-019 match implemented behavior
- [ ] T003 Verify non-functional requirements NFR-001 through NFR-008 match implementation
- [ ] T004 Review key entities table for completeness (check if any internal types are missing)
- [ ] T005 Remove implementation notes that do not belong in a specification document
- [ ] T006 Add any missing requirements discovered during review (e.g., error propagation semantics, thread-safety guarantees not captured)
- [ ] T007 Validate dependency table completeness (check if any transitive or implicit deps are missing)
- [ ] T008 Mark spec status as "Draft" or "Approved"

---

## Phase 2: Testing Gaps (Blocking - Required for Confidence)

**Purpose**: Fill coverage holes identified in plan.md. These tests validate existing code; no new features.

### Concurrency Tests

- [ ] T009 [US1] Add multi-threaded stress test: 16 threads performing concurrent insert/get/remove on overlapping key ranges in `src/lib.rs`
- [ ] T010 [US2] Add concurrent eviction test: multiple threads calling `evict_lru()` simultaneously while others insert, verifying no panics or double-free
- [ ] T011 Add Loom model-checking test for concurrent `initialize()` race (two threads calling initialize simultaneously)

### Eviction Tests

- [ ] T012 [US6] Add unit test for `evict_lru_for_key()`: insert keys into a specific shard, evict via targeted key, verify correct shard's LRU entry is removed
- [ ] T013 [US2] Add test verifying round-robin fairness: insert entries across all 16 shards, call `evict_lru()` 16 times, verify entries evicted from different shards
- [ ] T014 [US3] Add test verifying `touch()` promotion ordering with 3+ entries in same shard: insert A, B, C (all % 16 == same shard), touch A, evict from that shard, verify B evicted first

### Batch and Peek Tests

- [ ] T015 [P] [US7] Add unit test for `batch_touch()`: insert keys, batch_touch subset, verify eviction ordering reflects the promotion
- [ ] T016 [P] Add unit test for `oldest_keys(n)`: insert entries in known order across shards, verify returned keys are in oldest-first order

### Allocator Stress Tests

- [ ] T017 [P] Add fragmentation stress test in `src/allocator.rs`: random-sized alloc/dealloc cycles, verify capacity is fully recoverable after all deallocations
- [ ] T018 [P] Add property-based test for FreeList: after any sequence of allocate/deallocate, `used + sum(free_regions) == capacity`

### Edge Cases

- [ ] T019 [P] [US1] Add test for insert with size requiring rounding: insert size=1, verify used() reports 4096 (alignment rounding)
- [ ] T020 [P] [US8] Add test for `clear()` on already-empty pool: verify returns Ok(0)
- [ ] T021 [P] Add test for `pool_info()` before and after initialization
- [ ] T022 [P] Add test for `is_dma_capable()` returns false when SPDK feature is not active

---

## Phase 3: Documentation and Spec Alignment

**Purpose**: Ensure documentation, interface comments, and spec are mutually consistent.

- [ ] T023 [P] Verify `IMemoryTier` interface doc comments in `components/interfaces/src/imemory_tier.rs` match spec FR descriptions
- [ ] T024 [P] Verify Creusot property comments (P1-P10) in interface file are consistent with actual verification status
- [ ] T025 [P] Update `components/memory-tier/README.md` to reflect current spec content (pool size, shard count, alignment, API surface)
- [ ] T026 Verify `define_component!` version field ("0.2.0") matches `Cargo.toml` version ("0.1.0") -- resolve discrepancy

---

## Phase 4: Hardening and Improvement (Priority P2-P3)

**Purpose**: Address design considerations and known limitations identified during backfill.

### NUMA and Platform

- [ ] T027 [US4] Add integration test stub for NUMA binding verification (requires multi-socket hardware, mark `#[ignore]` with comment)
- [ ] T028 [US5] Add integration test stub for SPDK DMA path (requires SPDK env, mark `#[ignore]` with hardware note)

### Observability

- [ ] T029 Add per-shard metrics counters (inserts, gets, evictions, hit/miss) as optional instrumentation behind a feature flag
- [ ] T030 Add `Debug` or `Display` impl for `MemoryTierState` reporting pool_size, used, shard occupancy

### Robustness

- [ ] T031 Investigate and document clustered-key hot-shard behavior: add a benchmark showing throughput degradation when all keys map to same shard vs. uniform distribution
- [ ] T032 Add `#[cfg(test)]` helper to create a `MemoryTierComponent` with a configurable pool size (reduce test boilerplate in `setup()`)
- [ ] T033 [P] Audit all `unwrap()` calls on Mutex/RwLock in `IMemoryTier` impl -- verify poison-safety or document panic conditions

---

## Phase 5: Formal Verification Maintenance

**Purpose**: Keep Creusot proofs aligned with code changes.

- [ ] T034 Verify all 21 verification conditions still discharge after any code changes from earlier phases
- [ ] T035 [P] Add verification condition for `batch_touch` (currently unverified)
- [ ] T036 [P] Add verification condition for `clear` (only init-guard P2 is verified, not accounting reset)
- [ ] T037 [P] Add verification condition for `oldest_keys` ordering guarantee

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Review)**: No dependencies -- start immediately
- **Phase 2 (Testing Gaps)**: Can start in parallel with Phase 1; does not modify production code
- **Phase 3 (Documentation)**: Depends on Phase 1 completion (spec must be approved first)
- **Phase 4 (Hardening)**: Depends on Phase 2 (tests must exist before changes)
- **Phase 5 (Verification)**: Depends on Phase 4 (proofs must reflect final code)

### Parallel Opportunities

- All tasks marked `[P]` within a phase can run in parallel
- Phase 1 and Phase 2 can proceed concurrently (review is read-only; tests add new files)
- Within Phase 2, all test groups (Concurrency, Eviction, Batch, Allocator, Edge Cases) are independent

### Critical Path

```
T001-T008 (Review) ──► T023-T026 (Docs) ──► T029-T033 (Hardening) ──► T034-T037 (Verification)
     │
     └── T009-T022 (Tests, parallel) ──────────────────────────────────► T034 (re-verify)
```

---

## Notes

- This is a backfill task list -- the component is already implemented and passing tests
- Primary value: closing testing gaps, resolving the version discrepancy (T026), and approving the spec
- Phase 4 tasks are optional improvements; Phase 1 and Phase 2 are the immediate priorities
- Cargo.toml says version "0.1.0" but `define_component!` declares "0.2.0" -- this must be resolved in T026

# Feature Specification: Session-Lineage Eviction Policy

**Feature Branch**: `feat/component-eviction-policy-session-lists`

**Created**: 2026-08-03

**Status**: Draft

**Last-Synced**: 2026-08-20 (spec-sync Phase B; SC-001 re-verified against `benches/session_list_benchmark.rs` — still a design goal, no comparative harness)

**Input**: User description: "This component implements an eviction that is an alternative to LRU. For each new session id, a FILO list - or chain of blocks (stack), is used to track lineage of cache block (i.e., what block is the parent). When a block B for session id S is pushed immediately after block A, then it is known that block A is the parent of block B. Each block maintains a timestamp for the most recent time that it has been accessed. When eviction candidates are being requested, the algorithm selects (pops) the block from the session stack that has the oldest use timestamp (i.e. LRU from top of the stacks) - we are basically choosing from the leaves. This approach attempts to improve on basic LRU by exploiting lineage information to avoid loosing the head or higher up members of the chain. When a block is referenced, its timestamp is refreshed unless it is being evicted."

## Clarifications

### Session 2026-08-03

- Q: How is a session's lineage shaped — linear stack or branching tree? → A: Linear stack (each block has at most one child; exactly one leaf per session).
- Q: What is the eviction decision domain — shared across sessions or per-session? → A: Shared multi-session domain (victim is the globally oldest eligible leaf across all sessions in one domain; the caller conveys each block's session, extending `IEvictionPolicy`).
- Q: Should tracking state survive process restarts? → A: In-memory only (rebuilt by the cache on restart; persistence out of scope).
- Q: What happens when an already-tracked cache key is registered again? → A: Idempotent refresh (treated as an access to the existing block; no new node, lineage unchanged).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Select an eviction victim that preserves lineage (Priority: P1)

The caching subsystem is at capacity and needs to free space. It asks the policy which cache block to evict. The policy returns the *leaf* block (a block with no still-tracked child) whose most-recent access is oldest across all sessions it is tracking. Because only leaves are eligible, the head and interior members of a session's chain are protected until the blocks that descend from them have themselves been evicted.

**Why this priority**: This is the core value of the component and the reason it exists as an alternative to LRU. Without it, the component provides nothing the existing LRU policy does not. It is independently demonstrable on its own.

**Independent Test**: Register several sessions with multi-block chains, set differing access times, request a victim, and confirm the returned block is the oldest-accessed leaf and never an interior/head block that still has a tracked descendant.

**Acceptance Scenarios**:

1. **Given** two sessions, each with a chain of blocks, and the top (leaf) of session A is the oldest-accessed leaf overall, **When** a victim is requested, **Then** the policy returns and stops tracking that leaf of session A.
2. **Given** a session chain A → B → C (A is head, C is leaf), **When** a victim is requested and C's leaf is the oldest, **Then** C is chosen and A and B are not, **and** after C is evicted B becomes a leaf and thus eligible for a subsequent request.
3. **Given** an eviction domain with no tracked blocks, **When** a victim is requested, **Then** the policy reports that there is nothing to evict.
4. **Given** a request for up to N eviction candidates, **When** candidates are requested without eviction, **Then** the policy returns up to N leaves in eviction order and removes none of them.

---

### User Story 2 - Register a block into its session's lineage (Priority: P2)

When the cache admits a new block, it registers the block with the policy together with the session the block belongs to. The policy links the new block as the child of that session's current leaf, making the new block the session's new leaf. This records the parent→child lineage that the eviction decision later exploits.

**Why this priority**: Correct lineage is a prerequisite for the P1 behavior to be meaningful, but it delivers no observable value on its own until eviction uses it.

**Independent Test**: Register blocks A then B then C for one session and verify that the recorded chain is A (head) → B → C (leaf), and that a block registered for a different session starts a new, independent chain.

**Acceptance Scenarios**:

1. **Given** an empty session, **When** the first block is registered, **Then** that block is both the head and the leaf of the session's chain.
2. **Given** a session whose current leaf is A, **When** block B is registered for that session, **Then** A is recorded as B's parent and B becomes the leaf.
3. **Given** two different session ids, **When** blocks are registered under each, **Then** their chains are independent and neither appears in the other's lineage.

---

### User Story 3 - Refresh recency on access (Priority: P3)

When the cache reports that a block was accessed, the policy refreshes that block's most-recent-access timestamp to the current time, so that recently used blocks (and, by protecting their descendants' ordering, their lineage) are less likely to be chosen for eviction. A block is not refreshed at the moment it is being evicted.

**Why this priority**: Recency tracking tunes the quality of eviction decisions, but the policy still produces valid decisions using registration-time recency before this refinement is added.

**Independent Test**: Register two leaves, access one, request a victim, and confirm the non-accessed (older) leaf is chosen; then confirm that the block currently being evicted is not refreshed as a side effect of the eviction.

**Acceptance Scenarios**:

1. **Given** two leaves with equal initial recency, **When** one is accessed and a victim is then requested, **Then** the block that was **not** accessed is chosen.
2. **Given** a batch of accessed blocks reported together, **When** the batch is applied, **Then** every block in the batch has its recency refreshed.
3. **Given** a block that is selected for eviction, **When** it is evicted, **Then** its recency is not refreshed by the eviction itself.

---

### Edge Cases

- **Empty domain / empty session**: requesting a victim returns "nothing to evict"; requesting up to N candidates returns an empty list.
- **Single-block session**: the sole block is simultaneously head and leaf and may be evicted when it is the oldest leaf.
- **Tie in recency**: when multiple eligible leaves share the oldest timestamp, selection is deterministic and repeatable (stable ordering).
- **Access or removal after eviction**: acting on a block that is no longer tracked is reported as an invalid operation rather than silently succeeding.
- **Explicit removal of an interior block** (e.g., the cache frees a block directly): lineage stays consistent — the removed block's children are re-linked to the removed block's parent so no chain is orphaned.
- **Deep chains**: correctness does not depend on chain depth; a very long chain is handled the same as a short one.
- **Re-registration of an already-tracked block**: treated as an idempotent access to the existing block (recency refreshed); no new node is created and lineage is unchanged.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The policy MUST allow the caller to register a cache block together with the session it belongs to, and MUST return a handle that identifies the block for subsequent access-refresh and removal.
- **FR-002**: On registering a block for a session that already has blocks, the policy MUST record the session's current leaf as the new block's parent and MUST make the new block the session's leaf.
- **FR-003**: A block MUST belong to exactly one session; distinct sessions MUST be tracked as independent chains that never appear in one another's lineage.
- **FR-004**: The policy MUST record an access to a block by refreshing its most-recent-access timestamp to the current time.
- **FR-005**: The policy MUST support refreshing the recency of multiple blocks reported together in a single operation.
- **FR-006**: The policy MUST NOT refresh a block's recency as a side effect of evicting that block.
- **FR-007**: The policy MUST restrict eviction candidates to leaves (blocks with no still-tracked child); a block that still has a tracked descendant MUST NOT be selected for eviction.
- **FR-008**: When asked to identify the next victim, the policy MUST select, remove from tracking, and return the eligible leaf whose most-recent-access timestamp is oldest across all sessions in the eviction domain, or MUST report "nothing to evict" when the domain is empty.
- **FR-009**: After a leaf is evicted or removed, its parent MUST become a leaf and thus eligible for subsequent eviction.
- **FR-010**: The policy MUST allow the caller to request up to N eviction candidates, returned in eviction order, without removing any of them.
- **FR-011**: The policy MUST allow the caller to stop tracking any block by handle; if the block is interior to a chain, the policy MUST re-link its single child to its parent so the remaining chain stays consistent.
- **FR-012**: When multiple eligible leaves share the oldest timestamp, the policy MUST break the tie deterministically so repeated runs on identical state select the same victim.
- **FR-013**: The policy MUST report the number of blocks currently tracked in an eviction domain.
- **FR-014**: The policy MUST support clearing an eviction domain, returning it to empty.
- **FR-015**: Operations on an invalid or already-removed handle, or on a non-existent eviction domain, MUST be reported as errors rather than silently succeeding or corrupting state.
- **FR-016**: The policy MUST expose all of the above behavior exclusively through the shared eviction-policy interface; no capability may be reachable except through that interface.
- **FR-017**: Registering a cache key that is already tracked MUST be idempotent — treated as an access to the existing block (refreshing its recency) without creating a new node or altering lineage.
- **FR-018**: Each session's lineage MUST be a single linear chain (stack): every block has at most one child, and each session therefore has exactly one leaf (its stack top) while non-empty.

### Key Entities

- **Session**: A logical stream of related cache blocks identified by a session id. Owns a single linear chain (stack) of blocks from head (first registered, oldest ancestor) to leaf (most recently registered descendant). A non-empty session has exactly one leaf.
- **Cache Block**: A tracked unit of cached data identified by a cache key and a handle. Has at most one parent block and at most one child block, a most-recent-access timestamp, and a leaf/interior status.
- **Eviction Domain**: The shared collection of sessions among which a single eviction decision is made; the oldest-accessed leaf is chosen across all sessions in the domain. The caller conveys which session each block belongs to at registration.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001** *(design goal — not a measured outcome; downgraded 2026-08-07, re-verified 2026-08-20)*: On representative multi-turn session traces, the policy is intended to retain session-prefix (head/interior) blocks longer than basic LRU, reducing re-loads of retained-lineage prefixes relative to basic LRU on the same trace and capacity (design target: ≥15%). This is an aspirational design goal, **not** a verified measurable outcome. Verifying it requires a comparative trace-replay harness (session-lists vs `eviction-policy-lru` on a multi-turn ShareGPT-style trace, reporting prefix-reload counts); no such comparative benchmark exists in the repo. `benches/session_list_benchmark.rs` covers only this component's own hot-path throughput — its four Criterion benches (`track`, `touch`, `batch_touch`, `identify_next_to_evict`) measure per-operation cost through `IEvictionPolicy` at scale, never a cross-policy hit-rate comparison. Until such a harness exists, the ≥15% figure remains a design target, not a measured outcome. Building that harness is tracked as a follow-up in `.specify/sync/align-tasks.md` (Task 1).
- **SC-002**: Registering a block, refreshing recency, and stopping tracking of a block each complete in effectively constant time — their per-operation cost does not grow as the number of tracked blocks increases from thousands to at least one million.
- **SC-003**: Identifying the next victim completes in time that scales with the number of active sessions rather than the total number of tracked blocks, and remains bounded on a domain of at least one million blocks.
- **SC-004**: Victim selection is correct in 100% of cases: the policy never selects a block that still has a tracked descendant, and always selects the oldest-accessed eligible leaf (deterministically under ties).
- **SC-005**: The recency-refresh path sustains the cache hot-path access rate without becoming the bottleneck, measured against the batch-refresh throughput required by the caching subsystem.
- **SC-006**: After any sequence of register / access / evict / remove operations, the recorded lineage remains internally consistent (every non-head block has a tracked parent, no cycles, no orphaned children).

## Assumptions

- The session a block belongs to is supplied by the caller at registration time; the policy does not infer session membership from the cache key.
- "Current time" for recency is a monotonic time source available to the component; callers do not supply timestamps.
- Eligible eviction victims are session leaves only; head and interior blocks are protected until their descendants are evicted, which is the mechanism by which lineage is preserved. This is the intended reading of "choosing from the leaves / LRU from the top of the stacks."
- A single eviction decision compares leaves across all sessions in one shared eviction domain and returns the globally oldest eligible leaf, rather than being scoped to one session at a time (confirmed in Clarifications).
- Each session's lineage is a single linear stack, not a branching tree; a block has at most one child and each session has exactly one leaf (confirmed in Clarifications).
- Tracking state is held in memory only; it is not persisted and is rebuilt by the caching subsystem after a restart. Persistence and crash-consistency are out of scope for this component (confirmed in Clarifications).
- The component reuses the shared eviction-policy interface (`IEvictionPolicy`) defined in the `components/interfaces` crate. Because the current interface's registration call carries no session id, a session-aware registration path is added to that interface in `components/interfaces` rather than exposed as component-local public functions. The exact signature is resolved during planning.
- Hot-path operations (registration, recency refresh, removal, victim selection) may be invoked concurrently by the caching subsystem and must remain safe under the component framework's actor/interface model.
- The component maps its per-session chains onto the interface's existing "pool" concept, where a pool is the eviction domain that groups the sessions compared in one decision.
- The component targets Linux only, consistent with the project constitution.

## Observability *(non-normative, backfilled 2026-08-07)*

- On first selection as the active eviction policy, the component emits a
  one-time informational log line (guarded by an internal `announced` flag;
  `src/lib.rs:83-87,107-120`) via the optional `ILogger` receptacle. This is a
  startup-announcement diagnostic only — it is operationally useful for
  confirming which policy is bound, is not a functional requirement, and has no
  effect on eviction behaviour. A missing logger does not turn any operation
  into an error, consistent with the shared component logging convention.

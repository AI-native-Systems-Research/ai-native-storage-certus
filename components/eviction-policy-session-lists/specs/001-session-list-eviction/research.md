# Phase 0 Research: Session-Lineage Eviction Policy

All spec-level unknowns were resolved in `/speckit-clarify` (see spec Clarifications). This document records the remaining **design** decisions and their rationale.

## Decision 1 — How the session id reaches the policy

**Decision**: Extend the existing `IEvictionPolicy::track` in `components/interfaces` to take an additional `semantics: BlockSemantics` argument, **always passed by value** (not wrapped in `Option`), where `BlockSemantics` is a new, extensible hint struct carrying a required `session_id: SessionId` (with `pub type SessionId = u64`). Every tracked block is therefore unconditionally associated with a session. Existing implementations (`eviction-policy-lru`) and existing callers (`dispatch-map`, `memory-tier`) are updated mechanically: `eviction-policy-lru` **ignores** the `semantics` parameter, and session-unaware callers pass `BlockSemantics::default()` (`session_id = 0`). This component reads `semantics.session_id` to place the block into its session's chain. A caller that wants recency-LRU behavior from this component assigns each block a distinct `session_id` (e.g. the cache key), so each block becomes its own singleton chain.

**Rationale**:
- Keeps a **single** eviction-policy interface. Session awareness is a per-registration hint on the one `track` method, not a parallel interface that implementors must discover and consumers must down-cast to.
- `BlockSemantics` is **always present** and `session_id` is a required field, so there is no optionality to reason about at the call site — no `Option` wrapper and no optional field. This removes the confusing double-optional; the one degenerate case (recency-LRU) is an explicit caller choice (a distinct session per block) rather than an implicit `None` path.
- `BlockSemantics` is a struct rather than a bare `SessionId` so future per-block hints (priority, size class, ttl hint) can be added as new fields **without** re-touching the `track` signature or breaking callers. Policies ignore hints they do not use — `eviction-policy-lru` ignores `BlockSemantics` wholesale. `#[derive(Default)]` gives session-unaware callers a one-token value to pass.
- The `define_interface!` macro emits only method **signatures** (`#sig;`) and strips default bodies, so every implementor must carry the new parameter regardless; making it a signature change (not a new defaulted method) is therefore the honest, uniform way to evolve the trait. The change is mechanical for non-session policies (`_semantics` ignored).
- Honors Constitution Principle V: the capability is exposed only through the interface in `components/interfaces`, never as a component-local public function.
- `EvictionHandle`, `EvictionPolicyError`, and all other `IEvictionPolicy` methods are unchanged, so `touch`/`remove`/`identify_next_to_evict`/etc. work identically for every handle.

**Blast radius (accepted)**: `track` is called at 4 production sites — `dispatch-map` (3) and `memory-tier` (1) — plus test/bench sites; each gains a trailing `BlockSemantics::default()`. `eviction-policy-lru::track` gains an ignored `_semantics: BlockSemantics` parameter. This is a wider but purely mechanical, behavior-preserving change.

**Alternatives considered**:
- *Separate additive `ISessionEvictionPolicy` interface* (earlier plan revision): touches zero existing call sites, but proliferates interfaces and forces session-aware consumers to hold/resolve a second interface handle. Rejected in favor of one unified `track`.
- *`Option<BlockSemantics>` argument* (earlier plan revision): lets session-unaware callers pass `None`, but adds an optional path the component must branch on and reintroduces "no session" as an implicit interface state. Rejected in favor of an always-present `BlockSemantics` for a uniform call site.
- *Bare `SessionId` parameter instead of `BlockSemantics`*: simpler now but not extensible — any future hint would force another signature change across all implementors and callers. Rejected.
- *Encode session id inside `CacheKey`*: no interface change, but couples key layout to this one policy and breaks `IEvictionPolicy`'s opaque-key contract. Rejected.
- *One session per pool (`session == pool`)*: contradicts the clarified "shared multi-session domain". Rejected.

## Decision 2 — Core data structure

**Decision**: Per pool, an index-based arena `Vec<Node>` with a free-list (same pattern as `eviction-policy-lru::LruList`). Each `Node` holds `{ key, session: SessionId, parent: Option<u32>, child: Option<u32>, stamp: u64, active: bool }` (every block has a session; `parent == None` marks a chain head). Auxiliary per-pool maps: `by_key: HashMap<CacheKey, u32>` (idempotent re-registration + key→node), `sessions: HashMap<SessionId, u32>` (session → current leaf), and `leaves: BTreeSet<(u64, u32)>` ordering current leaves by `(stamp, index)`.

**Rationale**:
- Linear chains (per Clarifications) mean each node has **at most one child**, so parent/child `Option<u32>` links are sufficient — no child collections or tree bookkeeping.
- Eviction must pick the oldest-stamped leaf across sessions. A `BTreeSet<(stamp, idx)>` over **only** the leaves yields O(log S) victim selection and ordered candidate listing, where S = number of active sessions (one leaf per non-empty session). This satisfies SC-003 (scales with sessions, not blocks) and SC-002 (independent of total block count).
- `by_key` gives O(1) detection of an already-tracked key for FR-017 (idempotent refresh) and O(1) key→node lookup.
- Arena + free-list keeps handles stable (`index`) and allocation amortized, mirroring the proven LRU implementation.

**Alternatives considered**:
- *Linear scan of session leaves at eviction time*: makes register/touch/remove strictly O(1) but eviction O(S); rejected as primary because eviction on a large-S pool would be slower — `BTreeSet` keeps every operation O(log S) or better.
- *Binary heap of leaves*: no efficient decrease-key for the touch-a-leaf reorder; `BTreeSet` supports remove+reinsert cleanly; rejected.

## Decision 3 — "Most-recent-access timestamp" representation

**Decision**: Represent the timestamp as a per-pool monotonically increasing `u64` access counter (a logical clock), assigned on registration and on every access. Order leaves by `(stamp, index)`.

**Rationale**:
- A logical counter avoids syscalls/`Instant` on the hot path and gives a strict total order, so "oldest access" is unambiguous and tie-breaking (FR-012) is deterministic — distinct accesses get distinct stamps; `index` is a stable secondary key for safety.
- Wall-clock time is not required by any requirement; only *relative* recency ordering matters.

**Alternatives considered**:
- *`std::time::Instant`*: real time but adds per-op syscall cost and possible equal-instant ties; unnecessary. Rejected.

## Decision 4 — Concurrency model

**Decision**: `RwLock<EvictionState>` where `EvictionState { pools: Vec<Mutex<Pool>> }`; `create_pool` takes the write lock to append, all per-pool operations take the read lock then the pool's `Mutex`. `batch_touch` groups consecutive handles by pool to amortize lock acquisition (as `eviction-policy-lru` does).

**Rationale**: Matches the sibling component and the framework's interior-mutability requirement (interface methods take `&self`). Per-pool mutexes allow concurrent work across pools; the hot path (`touch`/`batch_touch`) never contends the write lock.

## Decision 5 — Removal semantics for interior blocks (FR-011)

**Decision**: `remove(handle)` untracks any node. Splice the linear chain: set `child.parent = node.parent` and `parent.child = node.child`. If the removed node was a leaf, drop it from `leaves` and, if its parent exists, the parent becomes a leaf (insert into `leaves`) and the session map points at the parent; if no parent remains, the session entry is dropped. `by_key` is updated and the slot freed.

**Rationale**: Preserves the single-linked-chain invariant (every non-head node has a live parent, no orphans/cycles — SC-006) under arbitrary removal, while keeping the `leaves` set exactly equal to the set of childless active nodes.

## Resolved unknowns summary

| Item | Resolution |
|------|-----------|
| Lineage shape | Linear stack (Clarifications) |
| Eviction domain | Shared multi-session pool; global oldest leaf (Clarifications) |
| Persistence | In-memory only (Clarifications) |
| Re-registration | Idempotent refresh (Clarifications) |
| Session delivery | Extended `IEvictionPolicy::track` with a by-value `BlockSemantics` (Decision 1) |
| Victim selection cost | O(log S) via `BTreeSet` over leaves (Decision 2) |
| Timestamp | Monotonic per-pool logical counter (Decision 3) |

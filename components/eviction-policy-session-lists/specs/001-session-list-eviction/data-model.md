# Phase 1 Data Model: Session-Lineage Eviction Policy

Derived from the spec's Key Entities and Functional Requirements. All state is in-memory (Clarifications). Types reference existing `components/interfaces` definitions plus one new type.

## Interface-level types (`components/interfaces`)

| Type | Definition | Source |
|------|-----------|--------|
| `PoolId` | `pub type PoolId = u32` | existing (`ieviction_policy.rs`) |
| `CacheKey` | `pub type CacheKey = u64` | existing (`idispatch_map.rs`) |
| `EvictionHandle` | `{ pool_id: u32, index: u32 }` | existing |
| `EvictionPolicyError` | `{ InvalidPool(PoolId), InvalidHandle }` | existing |
| `SessionId` | `pub type SessionId = u64` | **NEW (additive)** |
| `BlockSemantics` | `{ session_id: SessionId }` (required field), `derive(Default)` | **NEW (additive)** |

`SessionId` and `BlockSemantics` are added additively. `IEvictionPolicy::track` is
**extended** to `track(&self, pool, key, semantics: BlockSemantics)` — `semantics` is always
passed by value, never wrapped in `Option`. All other `IEvictionPolicy` methods and types are
unchanged. `identify_next_to_evict` returns `Option<CacheKey>` and `get_eviction_candidates`
returns `Vec<CacheKey>` (not handles).

`BlockSemantics` is **always supplied** and `session_id` is a **required** field, so every
tracked block is unconditionally associated with a session — there is no interface-level
"no session" case. Session-unaware callers pass `BlockSemantics::default()` (`session_id = 0`);
`eviction-policy-lru` ignores the argument entirely. To obtain recency-LRU behavior from *this*
component, a caller assigns each block a distinct `session_id` (e.g. the cache key) so each block
is its own singleton chain. `BlockSemantics` is an extensible hint bag: additional fields may be
added later without changing the `track` signature.

## Entities

### Eviction Domain → `Pool`

The eviction domain (FR-008, FR-013, FR-014) maps onto the interface's existing pool concept. One decision compares all leaves across all sessions in a pool.

Fields (per pool):

| Field | Type | Purpose |
|-------|------|---------|
| `nodes` | `Vec<Node>` | arena of tracked blocks; index = `EvictionHandle.index` |
| `free` | `Vec<u32>` | recycled slot indices (free list) |
| `by_key` | `HashMap<CacheKey, u32>` | key → node index; enforces idempotent re-registration (FR-017) |
| `sessions` | `HashMap<SessionId, u32>` | session id → current leaf node index (FR-002) |
| `leaves` | `BTreeSet<(u64, u32)>` | `(stamp, index)` of every current leaf, ordered oldest-first (FR-007, FR-008, FR-012) |
| `clock` | `u64` | monotonically increasing logical access counter (FR-004) |
| `len` | `usize` | count of active nodes (FR-013) |

### Cache Block → `Node`

A tracked unit (FR-001). Occupies one arena slot.

| Field | Type | Purpose |
|-------|------|---------|
| `key` | `CacheKey` | the cache key this node tracks |
| `session` | `SessionId` | owning session (every block has one; supplied via `BlockSemantics.session_id`) |
| `parent` | `Option<u32>` | index of parent block, `None` if head (FR-002) |
| `child` | `Option<u32>` | index of single child, `None` if leaf (FR-018: at most one child) |
| `stamp` | `u64` | most-recent-access logical timestamp (FR-004) |
| `active` | `bool` | slot occupancy flag (false → in `free`) |

### Session

Not a standalone struct — represented by an entry in `Pool.sessions` mapping `SessionId → leaf index`, plus the parent/child chain walkable from that leaf. A non-empty session has exactly one leaf (FR-018).

## Invariants (verified by property tests — SC-006)

1. **Single linear chain**: every active node has `child` referencing at most one node; following `parent` from any node terminates at a head (`parent == None`) without cycles.
2. **Leaf set exactness**: `leaves` contains `(stamp, idx)` for exactly the active nodes whose `child == None`.
3. **Session→leaf consistency**: for each `(sid → idx)` in `sessions`, `nodes[idx]` is active, `nodes[idx].session == sid`, and `nodes[idx].child == None`.
4. **No orphans**: for every active node with `parent == Some(p)`, `nodes[p]` is active and `nodes[p].child == Some(self)`.
5. **Key uniqueness**: `by_key` has exactly one entry per active node; no two active nodes share a key.
6. **Length agreement**: `len == count(active nodes) == nodes.len() - free.len()`.

## Key state transitions

| Operation | FR | Effect on model |
|-----------|----|-----------------|
| `track(pool, key, BlockSemantics{session_id: sid})` | FR-001, FR-002, FR-017 | If `key` in `by_key`: refresh its stamp (idempotent). Else allocate node with `session = sid`; if `sessions[sid]` exists, link as child of that leaf and remove old leaf from `leaves`; set node as new leaf in `sessions` + `leaves`; assign stamp. |
| recency-LRU usage: distinct `session_id` per block (e.g. `session_id = key`) | FR-001 | Each block gets a unique session → its own head+leaf singleton chain, always eligible; reproduces recency-LRU. No special interface path. |
| `touch(handle)` | FR-004 | If active: `clock += 1`, set node.stamp; if node is a leaf, remove old `(stamp,idx)` and reinsert new. Else `InvalidHandle`. |
| `batch_touch(handles)` | FR-005 | Apply `touch` per handle, grouped by pool. |
| `identify_next_to_evict(pool)` | FR-006, FR-007, FR-008, FR-012 | Pop smallest `(stamp,idx)` from `leaves`; splice out node; promote parent to leaf if it now has no child; return the evicted node's `CacheKey`; `None` if empty. Does not refresh recency. |
| `get_eviction_candidates(pool, n)` | FR-010 | Return up to `n` `CacheKey`s for the smallest `(stamp,idx)` leaves, in eviction order, removing none. |
| `remove(handle)` | FR-009, FR-011 | Splice node from chain (relink child.parent→parent, parent.child→child); update `leaves`/`sessions`/`by_key`; free slot. |
| `len(pool)` | FR-013 | Return `pool.len`. |
| `clear_pool(pool)` | FR-014 | Reset all pool collections to empty. |
| invalid pool / handle | FR-015 | Return `InvalidPool` / `InvalidHandle`. |

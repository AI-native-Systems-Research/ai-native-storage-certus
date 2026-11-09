# Contract: Extended `IEvictionPolicy::track` (session semantics)

**Location**: `components/interfaces/src/ieviction_policy.rs` (MODIFIED). `SessionId` and `BlockSemantics` re-exported from `components/interfaces/src/lib.rs`.

**Approach**: Session association is delivered as an optional per-registration hint on the **existing** `IEvictionPolicy::track` method, not through a separate interface. All other `IEvictionPolicy` methods and types are unchanged.

## New shared types

```rust
/// Identifies the session (logical stream of related cache blocks) that a
/// tracked block belongs to. Supplied by lineage-aware callers at registration.
pub type SessionId = u64;

/// Per-block semantic hints supplied to `track()` at registration.
///
/// Always passed to `track` by value. Extensible: new hint fields may be added
/// without changing the `track` signature or breaking existing callers /
/// implementations. Policies that do not use a given hint MUST ignore it (e.g.
/// `eviction-policy-lru` ignores `BlockSemantics` entirely). `Default` yields
/// `session_id = 0` for session-unaware callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockSemantics {
    /// Session this block belongs to, enabling lineage-aware eviction.
    /// Required: every tracked block is associated with a session. There is no
    /// interface-level "no session" case — session-unaware callers pass
    /// `BlockSemantics::default()`.
    pub session_id: SessionId,
}
```

## Modified method

```rust
// within define_interface! { pub IEvictionPolicy { ... } }

/// Register a key in the pool for eviction tracking. Returns a handle for
/// O(1) touch/remove.
///
/// `semantics` carries per-block hints (see `BlockSemantics`) and is always
/// supplied. Policies that do not use a hint MUST ignore it; session-unaware
/// callers pass `BlockSemantics::default()`. A lineage-aware policy reads
/// `semantics.session_id` to place the block into its session's chain.
///
/// Re-registering a key already tracked in `pool` is idempotent: it refreshes
/// that block's recency and returns its existing handle; no new node is
/// created and lineage is unchanged (FR-017).
fn track(
    &self,
    pool: PoolId,
    key: CacheKey,
    semantics: BlockSemantics,
) -> Result<EvictionHandle, EvictionPolicyError>;
```

## Behavioral contract (this component)

| # | Precondition | Guarantee | Maps to |
|---|--------------|-----------|---------|
| 1 | `pool` exists, `key` untracked, `semantics.session_id = s`, session `s` empty | New node is head+leaf of `s`; handle returned | FR-001, FR-002, User Story 2 §1 |
| 2 | `pool` exists, `key` untracked, session `s` has leaf L | L becomes new node's parent; new node becomes session leaf | FR-002, User Story 2 §2 |
| 3 | `pool` exists, `key` untracked, a distinct `session_id` per block | Each node is its own singleton head+leaf (reproduces recency-LRU) | FR-001 |
| 4 | `pool` exists, `key` already tracked | Idempotent: recency refreshed, existing handle returned, lineage unchanged | FR-017 |
| 5 | `pool` does not exist | `Err(InvalidPool(pool))`, no state change | FR-015 |
| 6 | Distinct `session_id` values | Chains are independent; neither appears in the other's lineage | FR-003, User Story 2 §3 |

## Impact on existing implementors and callers

The signature change is mechanical and behavior-preserving for non-session policies:

| Crate | Change |
|-------|--------|
| `eviction-policy-lru` (impl) | `track` gains `_semantics: BlockSemantics`, ignored; behavior identical |
| `dispatch-map` (caller ×3) | pass `BlockSemantics::default()` as the new argument |
| `memory-tier` (caller ×1) | pass `BlockSemantics::default()` as the new argument |
| test/bench call sites | pass `BlockSemantics::default()` (or an explicit `session_id`) |

## Error semantics

- `InvalidPool(pool)` — `pool` is not a live pool id.
- `InvalidHandle` — not returned by `track`; reserved for handle-consuming methods (`touch`, `batch_touch`, `remove`).

## Notes

- Recency-LRU behavior is obtained by giving each block a distinct `session_id` (e.g. the cache key), so each block is its own singleton chain — there is no separate "no session" call path.
- No new method is added; all lifecycle-after-registration operations reuse the existing, unchanged `IEvictionPolicy` methods.

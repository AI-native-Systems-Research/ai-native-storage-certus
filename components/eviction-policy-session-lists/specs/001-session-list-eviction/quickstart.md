# Quickstart: Session-Lineage Eviction Policy

## What this component is

`eviction-policy-session-lists` is a Certus eviction policy that improves on plain LRU by tracking per-session block **lineage**. Each session is a linear chain (stack) of cache blocks; only the leaf (stack top) of each session is eligible for eviction, and the victim is the globally oldest-accessed leaf across all sessions in a pool. This protects session prefixes (heads and interior blocks) that plain LRU would otherwise drop.

## Interfaces provided

- **`IEvictionPolicy`** — full lifecycle: `create_pool`, `track`, `touch`, `batch_touch`, `remove`, `identify_next_to_evict`, `get_eviction_candidates`, `len`, `clear_pool`. `track` is **extended** with a `semantics: BlockSemantics` argument (always by value); `BlockSemantics` carries a required `session_id`. Session-unaware callers pass `BlockSemantics::default()`; recency-LRU behavior comes from a distinct `session_id` per block.

No second interface is introduced: session awareness rides on the one `track` method via `BlockSemantics`.

## Typical usage

```text
1. create_pool()                                    -> pool_id
2. track(pool, keyA, BlockSemantics{session_id: S}) -> handleA (head+leaf of session S)
3. track(pool, keyB, BlockSemantics{session_id: S}) -> handleB (child of A; B is now leaf)
4. touch(handleB) / batch_touch([...])              -> refresh recency
5. identify_next_to_evict(pool)                     -> oldest leaf key, removed from tracking
6. remove(handle)                                   -> stop tracking a block, chain re-spliced

# recency-LRU: give each block its own session, e.g. track(pool, key, BlockSemantics{session_id: key})
# session-unaware caller: track(pool, key, BlockSemantics::default())
```

Session semantics:
- Registering blocks A then B then C under one session builds chain A(head) → B → C(leaf).
- After C is evicted, B becomes the session's leaf and is then eligible.
- Re-registering an already-tracked key just refreshes its recency (idempotent).

## Build, test, bench

```bash
# Component is a default workspace member (no SPDK dependency)
cargo build -p eviction-policy-session-lists
cargo test  -p eviction-policy-session-lists            # unit + doc + property tests
cargo test  -p eviction-policy-session-lists -- --test-threads 1   # CI mode

cargo bench -p eviction-policy-session-lists            # Criterion suite

# Interface crate (adds SessionId + BlockSemantics; extends IEvictionPolicy::track)
cargo build -p interfaces
cargo test  -p interfaces

# Quality gates (constitution)
cargo fmt --check
cargo clippy -p eviction-policy-session-lists -- -D warnings
cargo doc --no-deps -p eviction-policy-session-lists
```

## Where things live

| Path | Contents |
|------|----------|
| `components/interfaces/src/ieviction_policy.rs` | `SessionId` + `BlockSemantics` (additive); `track` extended with by-value `BlockSemantics` |
| `components/eviction-policy-lru/src/lib.rs` | `track` updated to ignore `_semantics` (mechanical) |
| `components/dispatch-map/src/lib.rs`, `components/memory-tier/src/lib.rs` | callers updated to pass `BlockSemantics::default()` |
| `components/eviction-policy-session-lists/src/lib.rs` | component facade, provides `IEvictionPolicy` (session-aware `track`) |
| `components/eviction-policy-session-lists/src/session_list.rs` | arena + per-session chains + leaf ordering |
| `components/eviction-policy-session-lists/benches/session_list_benchmark.rs` | Criterion perf suite |
| `components/eviction-policy-session-lists/tests/lineage_properties.rs` | lineage invariant / property tests (SC-006) |

## Success signals

- Victim selection never returns a block that still has a tracked child; always the oldest eligible leaf (SC-004).
- Register / touch / remove cost is independent of total block count; victim selection scales with active sessions (SC-002, SC-003).
- Lineage invariants hold after any operation sequence (SC-006).

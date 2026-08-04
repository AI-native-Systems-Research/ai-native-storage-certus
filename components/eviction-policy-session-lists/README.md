# eviction-policy-session-lists

## Summary

A Certus component implementing the `IEvictionPolicy` interface using a
**session-lineage** strategy. Instead of ranking blocks purely by recency (as
`eviction-policy-lru` does), this policy groups blocks into per-session chains
that record lineage, and only ever evicts the *leaves* of those chains. This
protects the shared prefix of a conversation — the blocks an inferencing session
keeps re-referencing — from being evicted while descendants still depend on it,
which raises cache-hit rate under memory pressure for prefix-sharing workloads.

The policy degrades gracefully to plain recency-LRU: if every block is given a
distinct `session_id`, each chain has length one, every block is its own leaf,
and victim selection reduces to "globally oldest-accessed block."

## Architecture

### Data Structure

Each pool tracks blocks in an index-based arena (`Vec<Node>` + free list),
mirroring `eviction-policy-lru::LruList`. Handles are `u32` arena indices, not
pointers, so nodes survive `Vec` reallocation and there are no self-referential
borrows.

Within a pool:

- **Session chains.** Each block belongs to exactly one session. Registering a
  block links it as the child of that session's current leaf, so the chain
  records lineage: the block pushed immediately before `B` in session `S` is
  `B`'s parent. A `session_id -> leaf index` map gives O(1) access to where the
  next block in a session attaches.
- **Leaf set.** A `BTreeSet<(stamp, index)>` holds every current leaf ordered
  oldest-access-first. Only leaves (blocks with no tracked child) are eligible
  for eviction. `identify_next_to_evict` pops the front of this set — the
  globally oldest-accessed leaf across all sessions — in O(log L), where `L` is
  the number of active sessions (each non-empty session contributes exactly one
  leaf). Removing a leaf promotes its parent to a leaf if the parent now has no
  children.
- **Logical clock.** A per-pool monotonically increasing counter stamps each
  access, giving a total order for leaf selection without wall-clock time.

Heads and interior blocks — those that still have descendants — are never
chosen as victims, which is the whole point: the prefix stays resident as long
as anything downstream of it does.

### Pool Isolation

Multiple independent pools are supported (each with its own arena, session map,
and leaf set). Pools are keyed by `PoolId` and guarded for concurrent access, so
callers such as the memory-tier and dispatch-map can maintain separate eviction
domains through one component instance.

## Interface

Provides `IEvictionPolicy`:

| Method | Purpose |
|---|---|
| `create_pool` | Create a fresh, empty eviction pool; returns its `PoolId`. |
| `track` | Register a block (idempotent); links it into its session's chain on first insert. |
| `touch` | Refresh a resident block's access stamp (re-orders it if it is a leaf). |
| `batch_touch` | `touch` several handles in one call. |
| `remove` | Stop tracking a block, mending its chain. |
| `identify_next_to_evict` | **Remove and return** the globally oldest-accessed leaf. |
| `get_eviction_candidates` | Peek at the oldest leaves without removing them. |
| `len` | Number of blocks currently tracked in a pool. |
| `clear_pool` | Drop all state for a pool. |

## Receptacles

| Receptacle | Interface | Purpose |
|---|---|---|
| `logger` | `ILogger` | Diagnostic logging. On first `create_pool` the component emits a one-time **info**-level line announcing it is the active eviction policy (visible at the default log level in `certus-server-yaml` output), followed by per-pool `debug` lines. |

## Usage

```rust
use component_core::query_interface;
use interfaces::{BlockSemantics, IEvictionPolicy};
use eviction_policy_session_lists::EvictionPolicySessionListsComponent;

let comp = EvictionPolicySessionListsComponent::new_default();
let ep = query_interface!(comp, IEvictionPolicy).unwrap();

let pool = ep.create_pool();
// Two blocks in the same session form a chain; the first (prefix) is protected.
let root = ep.track(pool, 0xA1, BlockSemantics { session_id: 7 }).unwrap();
let leaf = ep.track(pool, 0xB2, BlockSemantics { session_id: 7 }).unwrap();

ep.touch(root); // refresh the prefix

// Under pressure, the leaf is evicted before its still-referenced parent.
let victim = ep.identify_next_to_evict(pool);
assert_eq!(victim, Some(0xB2));
```

## Performance Test

The `eviction-replay-benchmark` app (`apps/eviction-replay-benchmark/`) replays a
captured **manager trace** through any `IEvictionPolicy` implementation and
reports, per cache size:

- **Effectiveness** — total cache **hits** / hit rate. A higher hit rate at the
  same cache size means the policy keeps the *important* (re-referenced) blocks
  resident longer — the core aim of session-lineage.
- **Performance** — mean per-call latency of the hot-path operations `touch` and
  `identify_next_to_evict` (plus `track`), and overall replay throughput.

The eviction policy holds no cache; the tool layers a fixed-capacity cache on top
of the policy and drives the trace's key references against it (resident key →
`touch`; absent key → evict via `identify_next_to_evict` until there is room,
then `track`). Trace block hashes are interned to dense `u64` keys, and each
operation's `session_id` is the interned id of its first block (the conversation
root), which this policy uses to protect the prefix.

### Running it

```bash
# Both policies, committed 199-prompt ShareGPT trace, several cache sizes:
cargo run --release -p eviction-replay-benchmark -- \
    --trace benchmarks/kv-offload-replay/traces/sharegpt/199-prompts.mgr.jsonl \
    --cache-size 16,32,64,128,256,442 \
    --policy both

# Just this policy:
cargo run --release -p eviction-replay-benchmark -- --policy session-lists

# Property/regression tests over the committed trace:
cargo test -p eviction-replay-benchmark
```

Flags: `--trace <FILE>` (`*.mgr.jsonl`), `--cache-size <N[,N…]>` (blocks),
`--policy lru|session-lists|both`.

### What it shows

On the 199-prompt ShareGPT trace (4,555 references, 442 distinct keys),
session-lineage converts far more references into hits under memory pressure than
plain LRU:

```
policy           cache      hits    hit%    evicts    touch(ns)    evict(ns)
lru                 32       536   11.8%      3987         51.8         52.5
session-lists       32      1277   28.0%      3246         63.8        149.6
lru                256      3773   82.8%       526         51.1         52.2
session-lists      256      3977   87.3%       322        130.5        189.8
```

The gain comes from protecting shared prefixes, at the cost of higher
per-operation latency from lineage bookkeeping. At a cache the size of the
working set (≥442) neither policy evicts and both reach the same 90.3% ceiling.
The advantage is workload-dependent: on traces with many short, interleaved
sessions and little prefix reuse the two policies converge at all cache sizes.

## Build & Test

```bash
cargo build -p eviction-policy-session-lists
cargo test  -p eviction-policy-session-lists
cargo clippy -p eviction-policy-session-lists -- -D warnings

# Criterion micro-benchmarks of the interface hot paths:
cargo bench -p eviction-policy-session-lists
```

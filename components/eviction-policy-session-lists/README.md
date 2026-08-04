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
of the policy and drives the trace's block references against it (resident block
→ `touch`; absent block → evict via `identify_next_to_evict` until there is room,
then `track`). The workload is a [Qwen-Bailian anonymized usage trace][qwen]:
`hash_ids` are globally-shared 16-token block ids used directly as keys, and each
request's `session_id` is its **conversation root** (`parent_chat_id` followed
transitively), so every turn of a conversation shares a session and this policy
protects the whole multi-turn prefix. Traces are named by short id
(`chat` / `api` / `thinking` / `coder`) and downloaded to `/tmp` on first use.

[qwen]: https://github.com/alibaba-edu/qwen-bailian-usagetraces-anon

### Running it

```bash
# Both policies, chat trace (default), several cache sizes:
cargo run --release -p eviction-replay-benchmark -- \
    --cache-size 256,1024,4096,16384,65536 --policy both

# Just this policy, on the coder trace:
cargo run --release -p eviction-replay-benchmark -- \
    --dataset coder --policy session-lists --cache-size 1024

# Offline property/regression tests (synthetic traces, no download):
cargo test -p eviction-replay-benchmark
```

Flags: `--dataset chat|api|thinking|coder` (default `chat`), `--file <PATH>`
(local Qwen-format JSONL, overrides `--dataset`), `--cache-size <N[,N…]>`
(blocks), `--policy lru|session-lists|both`.

### What it shows

On the `chat` trace (43,058 requests, 6.29M references, 2.53M distinct blocks),
session-lineage converts more references into hits under memory pressure than
plain LRU by keeping each conversation's shared prefix resident:

```
policy           cache      hits    hit%    evicts    touch(ns)    evict(ns)
lru                256    455891    7.2%   5837829         51.9         53.6
session-lists      256    515132    8.2%   5778588         32.2         93.1
lru               1024    809653   12.9%   5483299         31.7         33.3
session-lists     1024    818746   13.0%   5474206         34.2        100.1
lru              65536   2001497   31.8%   4226943         49.8         50.7
session-lists    65536   1930678   30.7%   4297762         36.9        125.5
```

The gain is largest when the cache is small relative to the working set, at the
cost of higher `identify_next_to_evict` / `track` latency from lineage
bookkeeping. As the cache approaches the working set the policies converge, and
once eviction is rare recency-only LRU can edge ahead. The effect is
workload-dependent — the `coder` trace shows a larger *relative* gain under
pressure (1.6% vs 0.3% hits at cache 256).

## Build & Test

```bash
cargo build -p eviction-policy-session-lists
cargo test  -p eviction-policy-session-lists
cargo clippy -p eviction-policy-session-lists -- -D warnings

# Criterion micro-benchmarks of the interface hot paths:
cargo bench -p eviction-policy-session-lists
```

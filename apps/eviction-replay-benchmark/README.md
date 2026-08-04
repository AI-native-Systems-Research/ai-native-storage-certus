# eviction-replay-benchmark

Replays a captured **manager trace** through any `IEvictionPolicy`
implementation and reports, for one or more cache sizes:

- **Effectiveness** — total cache **hits** / hit rate. Higher hit rate at the
  same cache size means the policy keeps the *important* (frequently
  re-referenced) blocks resident longer.
- **Performance** — mean per-call latency of the hot-path operations
  `touch` and `identify_next_to_evict` (and `track` for context), plus overall
  replay throughput.

It works for `eviction-policy-lru`, `eviction-policy-session-lists`, or any
component providing the `IEvictionPolicy` interface.

## How it works

The eviction policy only *decides what to evict*; it holds no cache. This tool
layers a fixed-capacity cache (`--cache-size` blocks) on top of the policy and
replays the trace's key references against it:

- every key reference is an **access**;
- a reference to a resident key is a **hit** → `touch`;
- a reference to an absent key is a **miss** → evict via
  `identify_next_to_evict` until there is room, then `track` the new key.

Trace block hashes (SHA-256 hex) are interned to dense `u64` `CacheKey`s. Each
operation's `session_id` is the interned id of its **first** block — the shared
prefix / conversation root — which lineage-aware policies use to protect a
conversation's prefix from eviction. Recency-only policies ignore it.

## Usage

```bash
# Both policies, default ShareGPT trace, three cache sizes:
cargo run --release -p eviction-replay-benchmark -- --cache-size 64,256,1024

# One policy, custom trace:
cargo run --release -p eviction-replay-benchmark -- \
    --policy session-lists \
    --trace benchmarks/kv-offload-replay/traces/sharegpt/199-prompts.mgr.jsonl \
    --cache-size 256
```

Flags: `--trace <FILE>` (`*.mgr.jsonl`, default is the committed 199-prompt
ShareGPT trace), `--cache-size <N[,N…]>` (blocks, default `256,1024,4096`),
`--policy lru|session-lists|both` (default `both`).

## Example (199-prompt ShareGPT trace: 4,555 refs, 442 distinct keys)

```
policy           cache      hits    hit%    evicts    touch(ns)    evict(ns)
lru                 32       536   11.8%      3987         51.8         52.5
session-lists       32      1277   28.0%      3246         63.8        149.6
lru                256      3773   82.8%       526         51.1         52.2
session-lists      256      3977   87.3%       322        130.5        189.8
```

Session-lineage converts far more references into hits under memory pressure
(protecting shared prefixes), at the cost of higher per-operation latency from
its lineage bookkeeping. At a cache the size of the working set (≥442) neither
policy evicts and both reach the same 90.3% ceiling.

## Tests

`tests/replay_hits.rs` drives the committed in-repo trace and checks the exact
no-eviction hit count, LRU's monotonic hit curve, the simulator's bookkeeping
invariants, and that the latency metrics are populated:

```bash
cargo test -p eviction-replay-benchmark
```

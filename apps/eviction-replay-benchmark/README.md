# eviction-replay-benchmark

Replays a real, captured LLM-serving trace through any `IEvictionPolicy`
implementation and reports, for one or more cache sizes:

- **Effectiveness** — total cache **hits** / hit rate. A higher hit rate at the
  same cache size means the policy keeps the *important* (re-referenced) blocks
  resident longer.
- **Performance** — mean per-call latency of the hot-path operations
  `touch` and `identify_next_to_evict` (and `track` for context), plus overall
  replay throughput.

It works for `eviction-policy-lru`, `eviction-policy-session-lists`, or any
component providing the `IEvictionPolicy` interface.

## Datasets

The workload is a [Qwen-Bailian anonymized usage trace][qwen] — two hours of
production KV-cache requests to a Qwen serving cluster. Four workloads are
available, selected by short id and **downloaded to `/tmp` on first use** (each
is tens to ~130 MB, git-LFS backed, so it is not committed here):

| id | workload | file |
|----|----------|------|
| `chat` | To-C interactive chat (multi-turn) | `qwen_traceA_blksz_16.jsonl` |
| `api` | To-B API-driven task automation | `qwen_traceB_blksz_16.jsonl` |
| `thinking` | reasoning-intensive chat | `qwen_thinking_blksz_16.jsonl` |
| `coder` | code generation | `qwen_coder_blksz_16.jsonl` |

Each line is one request: `{"chat_id", "parent_chat_id", "timestamp", "turn",
"type", "hash_ids": [...]}`. `hash_ids` are globally-shared 16-token block ids
(identical integer ⇒ identical cached block), used directly as `CacheKey`s.

[qwen]: https://github.com/alibaba-edu/qwen-bailian-usagetraces-anon

## How it works

The eviction policy only *decides what to evict*; it holds no cache. This tool
layers a fixed-capacity cache (`--cache-size` blocks) on top of the policy and
replays the trace's block references against it:

- every block reference is an **access**;
- a reference to a resident block is a **hit** → `touch`;
- a reference to an absent block is a **miss** → evict via
  `identify_next_to_evict` until there is room, then `track` the new block.

Each request's `session_id` is its **conversation root**: `parent_chat_id` is
followed transitively (`-1` marks a root) so every turn of a conversation shares
one session, giving lineage-aware policies the full multi-turn chain to protect.
Recency-only policies ignore it.

## Usage

```bash
# Both policies, chat trace (default), three cache sizes:
cargo run --release -p eviction-replay-benchmark -- --cache-size 256,1024,4096

# One policy on the coder trace:
cargo run --release -p eviction-replay-benchmark -- \
    --dataset coder --policy session-lists --cache-size 1024

# A local Qwen-format file instead of downloading:
cargo run --release -p eviction-replay-benchmark -- \
    --file /tmp/qwen_coder_blksz_16.jsonl --cache-size 4096
```

Flags: `--dataset chat|api|thinking|coder` (default `chat`), `--file <PATH>`
(local Qwen-format JSONL, overrides `--dataset`), `--cache-size <N[,N…]>`
(blocks, default `256,1024,4096`), `--policy lru|session-lists|both`
(default `both`).

## Example (`chat` trace: 43,058 requests, 6.29M refs, 2.53M distinct blocks)

```
policy           cache      hits    hit%    evicts    touch(ns)    evict(ns)
lru                256    455891    7.2%   5837829         51.9         53.6
session-lists      256    515132    8.2%   5778588         32.2         93.1
lru               1024    809653   12.9%   5483299         31.7         33.3
session-lists     1024    818746   13.0%   5474206         34.2        100.1
lru              65536   2001497   31.8%   4226943         49.8         50.7
session-lists    65536   1930678   30.7%   4297762         36.9        125.5
```

Under memory pressure (cache small relative to the working set) session-lineage
converts more references into hits by protecting each conversation's shared
prefix, at the cost of higher per-operation latency from its lineage
bookkeeping. As the cache grows toward the working set the policies converge,
and once eviction is rare recency-only LRU can edge ahead. The size of the
effect is workload-dependent — the `coder` trace shows a larger relative gain at
small caches (e.g. 1.6% vs 0.3% at cache 256).

## Tests

`tests/replay_hits.rs` runs fully offline on small synthetic Qwen-format traces
(no network): it checks conversation-root resolution, the exact no-eviction hit
count, LRU's monotonic hit curve, the simulator's bookkeeping invariants, and
that the latency metrics are populated:

```bash
cargo test -p eviction-replay-benchmark
```

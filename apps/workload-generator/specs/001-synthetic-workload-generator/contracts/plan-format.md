# Contract: Event Plan Artifact

**Version**: 1
**Status**: Draft
**Producers**: `certus-workload plan`, `certus-workload fit`
**Consumers**: `certus-workload simulate`, `certus-workload-run run`

The plan is the unit of reproducibility. Making it a persistable artifact — rather than
generating events inside the executor — is what lets the **identical** key stream be replayed
against the offline policy simulator and against real hardware, and lets two replacement
policies be proven to have seen the same input.

## Why the plan contains requests and not puts

A plan event says *"request R needs these keys, at these sizes, at this time, on this node."*
It does **not** say whether each key should be looked up or populated. The executor issues
lookups and populates whatever actually missed.

This is a correctness property, not a convenience. If the plan specified populates, it would
encode an assumed hit rate — the very quantity the experiment measures — and the same plan
would stop meaning the same thing under two different replacement policies. Keeping populates
derived makes the plan **policy-independent**, so a policy A/B is a controlled experiment.

## File layout

```
<name>.plan/
  manifest.json        # normalised YAML, content hash, corpus summary, partition index
  events.bin           # fixed-width event records, ascending timestamp
  events.<node>.bin    # optional pre-partitioned per-node slices
```

### `manifest.json`

```json
{
  "plan_format": 1,
  "generator_version": "certus-workload 0.1.0",
  "generator_build": "<git describe>",
  "content_hash": "blake3:...",
  "seed": 13421772,
  "normalised_yaml": "<the full input after extends-merge and defaulting>",
  "event_count": 10000000,
  "time_origin_ns": 0,
  "duration_ns": 180000000000,
  "corpus_summary": {
    "distinct_keys": 4210553,
    "total_bytes": 551724318720,
    "working_set_bytes": 68719476736,
    "wss_window_ns": 60000000000,
    "prefix_share_depth_histogram": [[0, 0.97], [8, 0.61], [24, 0.03]],
    "request_length_percentiles": {"p50": 22, "p90": 51, "p99": 118},
    "clamps_applied": {"depth_zero": 12, "normal_truncation": 0}
  },
  "partitions": {"node2": {"events": 2500431, "digest": "blake3:..."}},
  "stream_digest": "blake3:..."
}
```

`normalised_yaml` is embedded in full so a report can always be traced to its exact input,
including defaults that were applied rather than written.

### `events.bin` record

Fixed-width, little-endian, 32 bytes. Fixed width makes the file memory-mappable and
indexable by ordinal, and keeps event fetch allocation-free on the issuing path.

| Offset | Size | Field | Notes |
| --- | --- | --- | --- |
| 0 | 8 | `t_ns` | absolute nanoseconds from `time_origin_ns`; non-decreasing |
| 8 | 8 | `key` | the `CacheKey` (`u64`) |
| 16 | 4 | `size` | payload bytes; a pure function of `key` |
| 20 | 4 | `request_id` | groups the keys of one request; ascending |
| 24 | 2 | `node` | index into `topology.nodes` |
| 26 | 1 | `archetype` | tag, for per-archetype reporting |
| 27 | 1 | `flags` | bit 0 `REQUEST_START`, 1 `REQUEST_END`, 2 `WARMUP`, 3 `EXPECT_GLOBAL_MISS`, 4 `PIN`, 5 `HOLDER_TIER_SSD` |
| 28 | 4 | `depth` | trie depth of this key, for prefix-locality reporting |

10^7 events is 320 MB — routine, and streamable, so the whole plan is never required resident.

Keys of one request are contiguous and in path order, so a consumer batches a request by
scanning to `REQUEST_END` without buffering unboundedly.

## Determinism and distribution

- **FR-024/FR-026**: plan content is fully determined by the YAML plus `seed`, and the plan
  carries a `content_hash` over `manifest.json`'s normalised YAML plus `events.bin`.
- **Generated once, distributed, verified.** The orchestrator generates the plan, ships it,
  and every node verifies `content_hash` before executing its slice. Independent per-node
  generation was rejected: it would require pinning floating-point behaviour across compilers
  and CPUs, a hazard not worth taking on for a measurement tool.
- **`stream_digest`** is a rolling hash over `(key, size)` in event order. Both executors
  recompute it as they consume events and report it. A hit-rate comparison between two arms
  whose digests differ is refused (FR-062) — that is the mechanism that makes a policy A/B
  trustworthy rather than merely plausible.
- **Per-repeat seeds** derive from the root seed by a documented function of
  `(sweep_point_index, repeat_index)`, so an entire sweep is reproducible from one number.

## Non-decreasing time and open-loop lag

`t_ns` is non-decreasing, so the runner consumes the plan as a schedule. Under `open_loop` the
runner tracks cumulative lag between `t_ns` and actual issue time, and:

- reports cumulative and maximum lag,
- refuses to report the configured offered rate as achieved once lag exceeds a bound
  (FR-061),

because a slipped schedule means the run stopped measuring the load that was configured.

Under `closed_loop`, `t_ns` is advisory ordering only; the runner issues as sessions free up.

## Simulator equivalence

The simulator consumes the identical `events.bin`. It maintains modelled DRAM and SSD tiers of
the capacities in `system.capacity`, drives admission and eviction through `IEvictionPolicy`,
and classifies each event into the same five outcomes as the hardware runner
(`DRAM | SSD | REMOTE_DRAM | REMOTE_SSD | MISS`).

What the simulator deliberately does **not** model, and which therefore requires hardware to
measure (spec FR-035 requires this list be documented and kept current):

- RDMA transfer time, queue-pair contention, and connection establishment
- the zyre shout/quorum control plane and its `CERTUS_RL_QUORUM_PCT` timing
- SPDK NVMe queue depth, per-drive bandwidth, and poller-core effects
- GPU DMA bandwidth, PCIe topology, and cross-socket penalties
- dispatch-map lock contention and other real concurrency effects
- deadline expiry on negative lookups (`CERTUS_RL_OP_DEADLINE_MS`)

The simulator is therefore a *policy* screen — hit rate and eviction behaviour — never a
latency or throughput predictor. Its reports are marked accordingly so a simulated number can
never be mistaken for a measured one.

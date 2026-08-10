# Contract: Event Plan Artifact

**Version**: 1
**Status**: Draft
**Producers**: `certus-workload plan`; `certus-trace fit` produces the YAML a plan is generated from
**Consumers**: `certus-workload report | emit`, `certus-trace validate | convert`,
`certus-workload-run run`, and any external tool —
nothing in this format is specific to a Certus consumer.

The plan is the unit of reproducibility. Making it a persistable artifact — rather than
generating events inside the executor — is what lets the **identical** key stream be replayed
against anything at all, and lets any two arms be proven to have seen the same input.

## Why the plan contains requests and not puts

A plan event says *"request R needs these keys, at these sizes, at this time, on this node."*
It does **not** say whether each key should be looked up or populated. The executor issues
lookups and populates whatever actually missed.

This is a correctness property, not a convenience. If the plan specified populates, it would
encode an assumed hit rate — the very quantity the experiment measures — and the same plan
would stop meaning the same thing under two different replacement policies. Keeping populates
derived makes the plan **consumer-independent**, so an A/B between any two consumers — or between
two configurations of one — is a controlled experiment.

## File layout

```
<name>.plan/
  manifest.json        # normalised YAML, content hash, corpus summary, partition index
  events.bin           # fixed-width event records, ascending timestamp
  events.<node>.bin    # optional pre-partitioned per-node slices
```

## `manifest.json`

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
    "wss_window_requests": 240000,
    "prefix_share_depth_histogram": [[0, 0.97], [8, 0.61], [24, 0.03]],
    "request_length_percentiles": {"p50": 22, "p90": 51, "p99": 118},
    "roots": {
      "distinct_depth_zero_keys": 12,
      "sessions_per_root_histogram": [[0, 8134], [1, 5027], [2, 3311]]
    },
    "trunk_width_per_depth": [[0, 12], [4, 24], [18, 247], [40, 9967]],
    "trunk_occupancy_per_depth": [[0, 3334], [4, 1701], [18, 162], [40, 4.0]],
    "branching_resolved": [{"from_depth": 0, "fanout": 1.183}],
    "root_boundary_depth": 0,
    "churn_half_life_ns": 0,
    "churn_rotations": 0,
    "shared_depth_intended": [[4, 0.10], [18, 0.75], [40, 1.0]],
    "shared_depth_realised": [[4, 0.11], [18, 0.74], [40, 0.98]],
    "sessions": {
      "count": 41207,
      "per_wss_window": 40012,
      "turns_percentiles": {"p50": 4, "p90": 14, "p99": 31}
    },
    "clamps_applied": {"depth_zero": 12, "normal_truncation": 0}
  },
  "partitions": {"node2": {"events": 2500431, "digest": "blake3:..."}},
  "stream_digest": "blake3:..."
}
```

`normalised_yaml` is embedded in full so a report can always be traced to its exact input,
including defaults that were applied rather than written.

### `events.bin` record

Fixed-width, little-endian, 40 bytes. Fixed width makes the file memory-mappable and
indexable by ordinal, and keeps event fetch allocation-free on the issuing path.

| Offset | Size | Field | Notes |
| --- | --- | --- | --- |
| 0 | 8 | `t_ns` | absolute nanoseconds from `time_origin_ns`; non-decreasing |
| 8 | 8 | `key` | the `CacheKey` (`u64`) |
| 16 | 4 | `size` | payload bytes; a pure function of `key` |
| 20 | 4 | `request_id` | groups the keys of one request; ascending |
| 24 | 4 | `session_id` | the owning session; **not** derivable from `request_id` (see below) |
| 28 | 4 | `depth` | trie depth of this key, for prefix-locality reporting |
| 32 | 2 | `turn` | turn index within the session, 1-based |
| 34 | 2 | `node` | index into `topology.nodes` |
| 36 | 1 | `mix_index` | which `workload.mix` entry the session was drawn from |
| 37 | 1 | `flags` | bit 0 `REQUEST_START`, 1 `REQUEST_END`, 2 `WARMUP`, 3 `COLD`, 4-7 reserved, MUST be zero |
| 38 | 2 | `reserved` | MUST be zero; readers MUST reject non-zero |

Every field is naturally aligned and the record size is a multiple of 8, so an array of records
keeps the `u64` fields aligned without the decoder copying through a packed intermediate.

10^7 events is 400 MB — routine, and streamable, so the whole plan is never required resident.

Keys of one request are contiguous and in path order, so a consumer batches a request by
scanning to `REQUEST_END` without buffering unboundedly.

### Why `session_id` and `turn` are stored rather than derived

**Sessions interleave.** A session's turns are separated by `think_time`, during which other
sessions issue their own requests, so a session's requests are *not* contiguous in the plan and
session identity cannot be recovered from `request_id` grouping. Storing it is the only option,
and three things need it:

- Acceptance scenario 3 asserts that every session's requests begin with the same root key, and
  that keys below a session's own `shared_depth` are shared by no other session. Neither is
  checkable without knowing which session a key belongs to.
- `turn` is load-bearing in the path-length model: depth is
  `shared_depth + private_depth + Σ growth_per_turn` over turns (FR-014a), so `turn` is what
  makes a realised depth attributable to the model that produced it.
- Turn 1 and turn *N* are qualitatively different cache events — turn 1 walks a trunk someone
  else may have warmed, turn *N* re-reads blocks the same session just wrote — so a hit rate
  aggregated over both hides the effect under study.

`mix_index` replaces the former `archetype` tag: it preserves per-class reporting while the
schema itself has no `archetype` field (FR-014), because a mixture entry is a parameter set
rather than a behavioural mode.

`depth` is retained even though it equals the key's ordinal within its request, because deriving
it would require scanning back to `REQUEST_START` and so would defeat indexing by ordinal.

### Agent-fan-out lineage is derived, not stored

A spawned child (spec FR-018c) inherits its parent's prefix, and the record gains **no parent-session
field** for it. None is needed: keys are unique to their path, so a child's leading keys *are* the
parent's, and lineage is recovered by prefix-matching the key sequences. Grouping a report by fan-out
family means grouping by that shared prefix.

This is deliberate rather than a saving. A `parent_session` field would widen the record past 40 bytes
to 48 once aligned, for information already present; and it would be a second, independently-writable
statement of the same fact, so a plan could assert a lineage its keys contradict. The one thing that
must hold is that the child's inherited keys carry the **parent's** minting id (FR-009c), which is a
property of key derivation rather than of this format.

### Changing this record requires a `plan_format` bump

The record has no length prefix, so a decoder's only signal of the record width is
`manifest.json`'s `plan_format`. Any field added, removed, or resized MUST bump it, and readers
MUST refuse a `plan_format` they do not implement. The dispatcher's own wire codec is the
cautionary precedent: it frames by record count with no length prefix, so appending a field
there would mis-align an old decoder *silently*. The `reserved` bytes exist so that a
future flag or a widened `mix_index` can be added without moving any existing field.

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

## What this artifact does and does not carry

`events.bin` is a **block reference trace and nothing more**: for each event, which key, how
big, which request and session it belongs to, which node asks, and when. It carries no
capacities, no media, no tiers, no eviction policy, and no expected outcome for any event. That
is deliberate, and it is what makes one plan replayable against every consumer — a live Certus
server, a cache simulator with any number of levels, or a tool that merely counts distinct keys
— with the guarantee that each of them saw the identical stream (spec FR-036).

**Outcome classification belongs to the consumer, not to this artifact.** Where a block was
resolved from is something the consumer knows and reports; the plan cannot express it, and a
reader that infers a tier from a plan field is reading something that is not there. In
particular the `COLD` flag (bit 3) is *not* a predicted miss: it states only that the warmup
phase deliberately did not pre-request that key, which is a fact about the trace. Whether a
`COLD` key then misses is entirely up to the consumer, and a consumer that hits on one has not
violated anything.

Two consequences worth stating, because both were previously written into this contract as
though they were properties of the plan:

- A consumer with no independent store cannot produce a size disagreement, since size is a pure
  function of key identity (spec FR-011). If such a consumer reports one anyway, that is a
  **generator defect** — the one outcome that does reflect back on this artifact.
- Replaying this trace offline reproduces the *reference pattern* exactly and reproduces
  **nothing about time**. Fabric transfer and connection setup, control-plane quorum timing,
  device queue depth and per-device bandwidth, DMA bandwidth and interconnect topology, lock
  contention, and deadline expiry on absent keys are all invisible to a trace of this form. Any
  consumer that turns this artifact into a latency or throughput claim without hardware is
  making that claim up, and MUST say which of these it does not model. (Spec FR-035 used to require
  this; it was retired when offline replay left the suite, and the requirement now lives here, where
  it is a property of the artifact rather than of a tool that no longer exists.)

Any consumer that reports both kinds of number MUST mark which is which, so that a figure
derived from a model can never be mistaken for one that was measured.

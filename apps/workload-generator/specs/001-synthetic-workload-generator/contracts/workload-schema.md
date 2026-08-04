# Contract: Workload Model YAML Schema

**Version**: 1
**Status**: Draft
**Consumers**: `certus-workload plan | simulate | fit | validate`, `certus-workload-run run`

This is the normative reference for the generator's input. It is the contract that keeps the
input compact: the file holds *fitted statistical parameters*, never an access trace.

## Design rules

1. **Five orthogonal sections.** `corpus` (what keys exist and how they overlap), `workload`
   (who asks for what, when), `topology` (where copies live), `system` (the cache under
   test), `run` (execution and measurement). Changing one axis never requires editing
   another. This factoring is what prevents combinatorial enumeration.
2. **One distribution syntax everywhere** (§ Distributions).
3. **Reuse is specified exactly once**, inside a `workload.mix` archetype. A top-level
   `gets_per_key` or `lifetime` is a schema error, because the archetypes already imply
   reuse and a second specification would silently disagree with the first.
4. **No puts.** The model describes *requests*; populates are whatever the system missed.
   Specifying puts would assume the hit rate the experiment measures.
5. **Unknown fields are errors**, so a mistyped distribution parameter cannot silently take
   a default.
6. **Relative capacities.** Cache sizes are expressible as a fraction of the realised working
   set so a sweep means the same thing on machines with different DRAM.

## Top-level document

```yaml
version: 1                  # required; generator refuses versions it does not implement
seed: 0xC0FFEE              # required; every random draw derives from this
extends: presets/conv.yaml  # optional; deep-merged, this document wins
duration: 120s              # exactly one of duration | requests is required
requests: 2_000_000
corpus:   {...}             # required
workload: {...}             # required
topology: {...}             # optional; omitted ⇒ single node, no remote traffic
system:   {...}             # required
run:      {...}             # required
sweep:    {...}             # optional
```

### Units

Durations accept `ns|us|ms|s|m|h`. Sizes accept `B|KiB|MiB|GiB` (binary) and `KB|MB|GB`
(decimal). Rates accept a `/s` suffix. Bare integers may use `_` separators. Fractions are
plain floats in `[0, 1]` unless stated otherwise.

## Distributions

Every distribution-valued field takes the same tagged union. A bare scalar is sugar for
`{dist: const, value: <scalar>}`.

| `dist` | Parameters | Notes |
| --- | --- | --- |
| `const` | `value` | |
| `uniform` | `min`, `max` | inclusive |
| `normal` | `mean`, `stddev` | truncated at 0 for non-negative fields; truncation is counted and reported |
| `lognormal` | `median`, `sigma` | the default shape for sizes, lengths, and think times |
| `exponential` | `mean` | |
| `geometric` | `mean` | discrete; the default for turn counts |
| `zipf` | `s`, `n` | `s` is the exponent; `n` the support size |
| `pareto` | `scale`, `alpha` | |
| `empirical` | `points: [[value, cum_prob], ...]` | what `fit` emits when no parametric shape fits well; linearly interpolated |

Integer-valued fields round half-to-even and clamp to their documented domain; every clamp is
counted and surfaced in the plan summary (never silently applied).

## `corpus` — what keys exist and how they overlap

```yaml
corpus:
  # Payload size per key. MUST be a pure function of key identity: the generator
  # derives the draw from the key's own hash, never from position in the stream,
  # because the dispatcher treats a size mismatch as a miss and a varying size for
  # one key would manufacture phantom misses.
  block_bytes: {dist: const, value: 128KiB}

  prefix_tree:
    # Blocks per request == depth of the path walked through the trie.
    depth: {dist: lognormal, median: 24, sigma: 0.8}

    sharing:
      model: pitman_yor
      # Per-depth bands. The nearest *preceding* band applies to unlisted depths;
      # there is no interpolation. Band keys are the depth at which the band starts.
      #
      #   concentration (theta) -> mass on minting a NEW child. Low = everyone shares.
      #   discount (alpha)      -> tail heaviness among EXISTING children.
      #                            High = a few children absorb most descents.
      by_depth:
        0:  {discount: 0.10, concentration: 0.2}    # shared system prompt / few-shot preamble
        8:  {discount: 0.60, concentration: 8.0}    # task templates, conversation trunks
        24: {discount: 0.90, concentration: 500}    # each request's private suffix
```

### Why this generates prefix trees compactly

`CacheKey` is an opaque `u64`; in the vLLM path it is a rolling hash over the block chain.
So a shared prefix *is* a shared sequence of leading `u64`s, and the key space is a trie whose
node identity is the hash of its path. Key identity is therefore
`child_id = H(parent_id, child_index)` — the trie is never stored, and resident memory is
O(active paths) regardless of how many distinct keys a run mints.

A Pitman–Yor process over that trie gives realistic heavy-tailed sharing from two numbers per
band. Three bands, six numbers, reproduce the whole practical family:

| Band | `concentration` | Effect |
| --- | --- | --- |
| near-root | → 0 | Almost every request shares the same first blocks — a global system prompt |
| mid-depth | moderate | A moderate number of trunks — task templates, conversations |
| deep | → ∞ | Almost every key is novel — the request's own tokens |

Degenerate settings are detected: a configuration that can never mint a new child makes the
key space finite and the run meaningless, and is reported rather than run.

## `workload` — who asks for what, when

```yaml
workload:
  arrival:
    model: open_loop            # open_loop (default) | closed_loop
    rate: 4000/s                # open_loop only; distribution-valued
    burstiness: 1.8             # index of dispersion; 1.0 == Poisson (neutral value)
    concurrency: 256            # closed_loop only: bounded in-flight sessions

  # Weighted mixture. Weights are normalised, not required to sum to 1.
  mix:
    - archetype: conversation
      weight: 0.70
      turns: {dist: geometric, mean: 6}
      think_time: {dist: lognormal, median: 3s, sigma: 1.1}
      growth_blocks_per_turn: {dist: lognormal, median: 6, sigma: 0.5}

    - archetype: one_shot
      weight: 0.25
      popularity: {dist: zipf, s: 0.9, n: 50_000}

    - archetype: scan
      weight: 0.05
      length_blocks: {dist: const, value: 4000}
      novel_fraction: 0.98

  drift:
    half_life: 300s             # popularity non-stationarity; 0 (default) = stationary
```

### `open_loop` vs `closed_loop` — this choice affects validity

Under `closed_loop`, arrival times depend on how fast the system responds, so **two
replacement policies see different key streams** and any hit-rate comparison between them is
confounded by the system's own speed. Use:

- **`open_loop`** (default) for hit-rate and policy comparison — absolute timestamps keep the
  key stream identical across arms. The runner reports cumulative schedule lag, and will not
  claim a configured offered rate was achieved when the schedule slipped.
- **`closed_loop`** for throughput and saturation measurement, where queueing is the
  phenomenon of interest.

### Archetypes

| Archetype | Reuse mechanism | Models |
| --- | --- | --- |
| `conversation` | Turn N+1 re-reads turn N's blocks, then extends the path by `growth_blocks_per_turn`; gets separated by `think_time` | Multi-turn chat. Recency-friendly — the dominant real KV-cache pattern |
| `one_shot` | Single request; reuse arises from `popularity` over shared prefixes | Independent requests over a shared preamble. Frequency-friendly |
| `scan` | Essentially none (`novel_fraction` → 1) | Long-document ingest. The classic LRU-polluting case |

Reuse lives **only** here. There is no top-level `gets_per_key` or `lifetime`; supplying one
is a schema error (spec FR-007).

## `topology` — where copies live

```yaml
topology:
  nodes: [node2, node7, node9, node11]

  # Probability the node serving a request already holds the key locally.
  # 1.0 = never remote; 0.0 = always remote. The single most useful remote knob.
  self_affinity: 0.25

  replication:
    holders_per_key: {dist: const, value: 1}

  # Forced tier of the authoritative copy, realised via the existing
  # FlushToSsd / ClearMemoryTier RPCs during plan setup.
  holder_tier: {dram: 0.7, ssd: 0.3}

  # Keys no node holds -> exercises the negative-lookup path, which burns the
  # full CERTUS_RL_OP_DEADLINE_MS (50ms default) per miss.
  global_miss_fraction: 0.05

  membership_events:
    - {at: 60s, action: stop,  node: node9}
    - {at: 90s, action: start, node: node9}
```

**Request placement is uniform across nodes and there are no requester or holder roles.**
Every node both requests and holds. Real cross-node KV copies are essentially symmetrical, so
role assignment would model the lab rather than the deployment. Hardware asymmetry is handled
by `preflight` refusing to run (spec FR-049..FR-053), not by steering load.

## `system` — the cache under test

```yaml
system:
  capacity:
    # fraction_of_wss is preferred: it makes a sweep mean the same thing across
    # machines with different DRAM. `bytes` is accepted for absolute pinning.
    dram: {fraction_of_wss: 0.25}
    ssd:  {fraction_of_wss: 1.5}
  eviction_policy: lru          # component providing IEvictionPolicy
  pin_fraction: 0.0             # fraction of live entries held pinned
  thresholds:                   # optional; maps onto DispatcherConfig
    memory_tier_eviction: {high: 0.80, low: 0.70}
    ssd_eviction:         {high: 0.90, low: 0.80}
```

The working set size used by `fraction_of_wss` is computed from the realised plan over
`run.wss_window` and recorded in the plan summary, so the absolute capacity a given run used
is always recoverable from its report.

## `run` — execution and measurement

```yaml
run:
  mode: hardware              # hardware | simulate
  endpoint_template: "{node}:50051"
  batch_size: 64              # keys per RPC
  workers: 8                  # client threads
  inflight: 4                 # concurrent RPCs per worker
  gpu_buffer: 8GiB            # one process-wide CUDA allocation, addressed by offset
  warmup: 20s                 # excluded from steady-state statistics
  warm_connections: true      # explicit RDMA connection-warm phase before measuring
  wss_window: 60s             # window for the working-set-size calculation
  clock_skew_bound: 1ms       # preflight fails above this
  emit_trace: /tmp/plan.jsonl # optional, debugging only; never an input
```

## `sweep` — the experiment matrix

```yaml
sweep:
  axes:
    topology.self_affinity: [0.0, 0.25, 0.5, 1.0]
    system.capacity.dram.fraction_of_wss: [0.1, 0.25, 0.5, 1.0]
  policies: [lru, arc]        # optional extra axis over system.eviction_policy
  repeat: 8                   # default 8
  order: interleaved          # interleaved (default) | blocked
```

Axes form a cartesian product. Dotted paths address any scalar in the document. Each
`(point, repeat)` gets a seed derived deterministically from the root `seed`, so an entire
sweep is reproducible from one number.

`repeat` defaults to **8** because prior measurement on this bench established that n = 3
produced misleading conclusions and n ≥ 8 is needed for significance. `order: interleaved`
rotates through points across repeats rather than completing all repeats of one point first,
so slow environmental drift does not alias onto a single sweep point.

## Presets and `extends`

`extends` deep-merges a base document; the including document wins on every conflicting leaf.
Lists replace rather than append. This is what delivers the compactness target: a common
experiment should be under ten lines.

```yaml
extends: presets/conversational-multinode.yaml
seed: 7
sweep:
  axes: {topology.self_affinity: [0.0, 0.25, 0.5, 1.0]}
  repeat: 8
```

Presets to ship, one per Test Matrix family:

| Preset | Shape |
| --- | --- |
| `presets/zipf-baseline.yaml` | No prefix tree, pure Zipf. Harness validation — LRU hit rate is analytic |
| `presets/conversational.yaml` | `conversation` 1.0. Recency-friendly |
| `presets/shared-preamble.yaml` | `one_shot` 1.0 over a `concentration → 0` root band. Frequency-friendly |
| `presets/mixed.yaml` | The mixture, set up for a weight sweep. The headline experiment |
| `presets/scan-pollution.yaml` | Hot conversational set plus 5% `scan` |
| `presets/conversational-multinode.yaml` | `presets/conversational.yaml` plus a 4-node `topology` |
| `presets/global-miss-storm.yaml` | High `global_miss_fraction`, for negative-lookup cost |
| `presets/fitted-sharegpt.yaml` | Emitted by `fit` against the checked-in ShareGPT trace |

## Validation rules

The generator rejects, rather than silently accepting:

1. Unknown fields anywhere in the document.
2. A `version` it does not implement.
3. Reuse specified outside a `workload.mix` archetype (FR-007).
4. Any populate/put specification (FR-023).
5. Both `duration` and `requests`, or neither.
6. A `mix` with no entries, or all weights zero.
7. Distribution parameters outside their domain (`zipf.s <= 0`, `sigma < 0`, negative sizes,
   fractions outside `[0, 1]`).
8. `by_depth` bands not in ascending depth order, or with no band starting at depth 0.
9. A Pitman–Yor configuration that can never mint a new child (finite, degenerate key space).
10. `holder_tier` fractions that do not sum to 1.0 within tolerance.
11. `replication.holders_per_key` exceeding `len(topology.nodes)`.
12. `topology.membership_events` referencing a node not in `topology.nodes`, or an `at` beyond
    `duration`.
13. `pin_fraction` high enough to drive effective capacity to zero.
14. `mode: hardware` with no `topology.nodes` and no endpoint.
15. `sweep.axes` dotted paths that do not resolve to a scalar in the document.

## Worked example — the headline mixture experiment

Complete and runnable; 44 lines including comments.

```yaml
version: 1
seed: 0xC0FFEE
duration: 180s

corpus:
  block_bytes: 128KiB
  prefix_tree:
    depth: {dist: lognormal, median: 24, sigma: 0.8}
    sharing:
      model: pitman_yor
      by_depth:
        0:  {discount: 0.10, concentration: 0.2}
        8:  {discount: 0.60, concentration: 8.0}
        24: {discount: 0.90, concentration: 500}

workload:
  arrival: {model: open_loop, rate: 4000/s, burstiness: 1.8}
  mix:
    - {archetype: conversation, weight: 0.70,
       turns: {dist: geometric, mean: 6},
       think_time: {dist: lognormal, median: 3s, sigma: 1.1},
       growth_blocks_per_turn: {dist: lognormal, median: 6, sigma: 0.5}}
    - {archetype: one_shot, weight: 0.25,
       popularity: {dist: zipf, s: 0.9, n: 50_000}}
    - {archetype: scan, weight: 0.05, length_blocks: 4000, novel_fraction: 0.98}

topology:
  nodes: [node2, node7, node9, node11]
  self_affinity: 0.25
  replication: {holders_per_key: 1}
  holder_tier: {dram: 0.7, ssd: 0.3}
  global_miss_fraction: 0.05

system:
  capacity:
    dram: {fraction_of_wss: 0.25}
    ssd:  {fraction_of_wss: 1.5}
  eviction_policy: lru

run:
  mode: hardware
  batch_size: 64
  workers: 8
  inflight: 4
  warmup: 20s

sweep:
  axes: {system.capacity.dram.fraction_of_wss: [0.1, 0.25, 0.5, 1.0]}
  policies: [lru]
  repeat: 8
```

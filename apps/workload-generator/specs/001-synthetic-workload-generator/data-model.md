# Phase 1 Data Model: Synthetic Workload Generator

**Feature**: `001-synthetic-workload-generator` | **Date**: 2026-09-15

Entities as they exist inside `workload-model`, the CUDA-free crate both
execution paths consume. Field names are indicative; the invariants and state
transitions are the normative part. Requirement references point at `spec.md`.

## Layering

```text
WorkloadDescription  (parsed YAML, validated once)
        │
        ├── SharedClass ──> SharedPool ──> SharedInstance
        │
        └── SessionClass ─> SessionPool ─> Session ──> Turn
                                              │
                                              └──> PrefixChain ──> CacheKey
                                                          │
                                                    OperationPlan ──> Operation
```

Everything above the `OperationPlan` line is virtual-time simulation and is
identical in a live run and an emit run (FR-072). Everything below it is
execution: lanes, transport, containers.

## Distribution

The one sampling primitive. Every numeric field in the description is one of
these.

| Field | Type | Notes |
| --- | --- | --- |
| `kind` | enum | `Constant`, `Uniform`, `Empirical`, `Normal`, `Exponential` |
| `params` | per-kind | e.g. `mean`/`sigma`, `min`/`max` bounds |
| `bounds` | `Option<f64>` × 2 | truncation bounds, absent = unbounded |
| `integral` | bool | set by the *consumer*, not the file |

**Invariants**

- Truncation is inverse-transform between `F(min)` and `F(max)`, never clamping
  (FR-008). Clamping would pile mass at a boundary and shift the mean.
- When `integral`, the effective bounds are `[min − 0.5, max + 0.5]` and the
  draw rounds to nearest, so every integer in range carries equal weight
  (FR-009). This is a property of the *consumer's* type, not of the written
  distribution — `uniform{0,5}` for a count is six equiprobable values, while
  the same distribution for a lifetime is continuous.
- `inf` is legal in any numeric position (FR-011).
- `Empirical` carries samples inline or from a file, and is discrete by default
  for counts (FR-010). Interpolating a bimodal sample set places mass in gaps
  the data says are empty; permitted, but not the default where it matters.
- Every distribution can report its **effective** mean and quantiles after
  truncation, which is what FR-003 reports at load and FR-004 gates on.

## WorkloadDescription

Parsed once, validated once, immutable thereafter.

| Field | Notes |
| --- | --- |
| `version` | schema version; unknown versions rejected (FR-006) |
| `blocks.tokens`, `blocks.bytes` | block geometry |
| `shared_classes` | name → `SharedClass` |
| `session_classes` | name → `SessionClass` |

**Validation, all before any operation is issued (FR-002)**

1. Every `uses` entry names a declared shared class.
2. For each `uses` entry, the count distribution truncated to the referenced
   pool's size must not discard more than 5% of its mass (FR-004). The error
   names both the requested and effective mean.
3. A class with an unbounded lifetime may not use the `poisson` population form
   — the birth rate `N / E[lifetime]` is undefined (FR-017 makes it a mint-once
   pool).
4. Effective distributions are reported for every field whose truncated value
   differs from what was written (FR-003).
5. No host-specific or tuning field appears anywhere in the file (FR-005).

## SharedClass and SharedPool

| `SharedClass` field | Notes |
| --- | --- |
| `length` | Distribution, integral — blocks per instance |
| `lifetime` | Distribution — virtual seconds, may be `inf` |
| `pool.size` | `Exact(n)` \| `Poisson(n)`, default `Poisson` |
| `pool.rank_by` | `Slot` \| `Recency` |
| `pool.selection` | `Option<Distribution>` over instance index |

`SharedPool` holds the live instances and is the only place population dynamics
live.

**State transitions**

```text
                  ┌─────────────── Exact ───────────────┐
   (t=0) seed from residual-life ──> Live ──death──> replaced immediately
                  └─────────────── Poisson ──────────────┘
   (t=0) seed from residual-life ──> Live ──death──> gone; births are a
                                                     free-running process at
                                                     rate N / E[lifetime]
   Live ──lifetime expires──> Retired (unselectable) ──last user ends──> Released
```

**Invariants**

- Seeding at `t = 0` draws from the **equilibrium residual-life** distribution,
  never from `lifetime` itself (FR-015). Seeding from `lifetime` gives every
  instance the same birthday; measured on a normal-lifetime pool that produced
  zero key churn for the first 6000 virtual seconds followed by a burst, with
  ~40% peak-to-trough modulation still present after eight generations.
- No feedback controller and no gain parameter exists (FR-016).
- Retirement removes an instance from *selection* only; sessions already
  holding it keep using it, and its bookkeeping is released when the last such
  session ends. Certus is told nothing (FR-018).
- `rank_by: Slot` — index is a pool slot; a new instance inherits the slot's
  popularity and holds it for life. `rank_by: Recency` — index 0 is newest, so
  heat decays as newer instances arrive. Under `Recency` an instance goes cold
  long before its lifetime expires, which makes `lifetime` nearly inert; under
  `Slot`, `lifetime` is the only thing that retires a key. Same file, different
  parameter doing the work (FR-020).
- Index space must stay bounded. Numbering by a monotonic mint counter is
  **wrong**: popularity would attach permanently to the earliest instances and
  the pool would mint objects nothing selects.

## SharedInstance

| Field | Notes |
| --- | --- |
| `index` | selection probability *and* prefix position |
| `length_blocks` | drawn once at creation |
| `born_at`, `dies_at` | virtual seconds |
| `users` | refcount, for the release transition |

## SessionClass and SessionPool

| `SessionClass` field | Notes |
| --- | --- |
| `uses` | **ordered** list of `(class, count Distribution)` |
| `turns` | Distribution, integral |
| `input_growth`, `output_growth` | Distribution, integral — blocks per turn |
| `think_time` | Distribution — virtual seconds before each turn |
| `migration_interval` | `Option<Distribution>` |
| `pool.size` | `Exact(n)` \| `Poisson(n)`, concurrent sessions |

`pool.size` means concurrent sessions, so the arrival rate is an *output*,
settling at `size / E[session lifetime]` by Little's law (FR-024).

## Session

| Field | Notes |
| --- | --- |
| `id` | |
| `class` | |
| `chosen` | resolved shared instances, in canonical order |
| `prefix` | `PrefixChain` |
| `turn_cursor`, `turns_total` | |
| `node` | current placement |
| `next_migration_at` | |

**Canonical ordering (FR-028)** — the whole reason sharing works:

1. Shared classes in the order the session class lists them.
2. Instances **sorted by index** within a class.
3. Drawn without replacement, bounded by `min(nominal pool size, live count)`
   (FR-022), so the draw always succeeds with no wait and no failure path.

Sorting makes the prefix a pure function of the chosen *set*, so two sessions
with overlapping sets produce **nested** chains rather than divergent ones. A
session drawing `{0,1}` and one drawing `{0,1,4}` share their first two
objects' blocks; without sorting they might share nothing.

**State transitions**

```text
Created ──think_time──> Turn 1 ──think_time──> Turn 2 ── … ──> Turns exhausted ──> Ended
   │                                                                                 │
   └── migration_interval elapses ──> node reassigned (uniform among others) ─────────┘
        blocks stay on the old node; the new node must fetch remotely (FR-048)
```

## Turn

| Field | Notes |
| --- | --- |
| `index` | 0-based within the session |
| `at` | virtual seconds |
| `reads` | the whole prefix so far, in order |
| `new_input`, `new_output` | freshly minted blocks |

**Invariants**

- Append-only: turn *n* reads its shared objects plus every input and output
  block minted by turns `1..n-1`, so per-turn work grows linearly with turn
  index and total work is quadratic in session length (FR-025).
- Both input and output blocks are stored and both extend the chain (FR-026).
- A turn occupies a single virtual instant. Virtual time advances only through
  think time, so concurrency at execution is a lane-count choice, not a
  property of the plan.

## PrefixChain and CacheKey

`CacheKey` is `u64`, opaque to Certus
(`components/interfaces/src/idispatch_map.rs:6`), so the generator owns the key
space.

| Field | Notes |
| --- | --- |
| `keys` | `Vec<CacheKey>`, chain order |
| `tip` | the last key, the parent of the next |

**Invariants**

- Chained: a block's key derives from its own identity **and** its parent key
  (FR-027). Reuse between sessions therefore requires a matching leading run;
  an object held in common at a differing position yields nothing.
- Derived by the specified splitmix64 mix over (parent key, salt) — see
  `contracts/key-derivation.md`. Not a general-purpose hasher: `DefaultHasher`
  (SipHash) and `ahash` are unstable across versions, which would leave a
  trace verifiable only by the exact binary that wrote it (FR-029). Deriving
  rather than minting is what makes a prefix stateless; it is *not* about
  cross-node agreement, since one generator owns the whole simulation.
- Growth blocks chain onto the session's unique prefix, making them private by
  construction and reusable only intra-session (FR-030). This is what makes the
  two sources of cache hits separable.
- Appending is O(1); a turn's read list is a slice of already-computed keys, so
  the quadratic cost is in *iteration*, never in rehashing.

## OperationPlan and Operation

The boundary between simulation and execution, and the artifact FR-034 and
SC-003 assert byte-identity against.

| `Operation` field | Notes |
| --- | --- |
| `at` | virtual seconds |
| `session` | for the per-session ordering constraint |
| `node` | placement at the time of the operation |
| `kind` | `Check` \| `Load` \| `Reserve` \| `Transfer` \| `Commit` \| `Abort` \| `PollEvents` |
| `keys` | in prefix order |

**Invariants**

- Totally ordered by `at`, then deterministically by session id to break ties.
- A pure function of description and seed — independent of server speed, of
  execution mode, and of request batching and lane count (FR-072). Batching
  groups operations *on the wire*; it never changes what they are.
- Per-session order is strict; cross-session operations may overlap, including
  two sessions racing to mint the same shared prefix (FR-035). That race is
  faithful to production, so hit/miss outcomes are not reproducible even though
  the plan is (FR-036).
- The store sequence is reserve → transfer → commit-or-abort, never a
  single-shot store, because the production client has no such operation
  (FR-040).

## Execution-side entities

Not part of the deterministic core; present so the boundary is explicit.

| Entity | Notes |
| --- | --- |
| `Lane` | one unit of execution concurrency, holding one session's turn at a time. Count must not exceed the server's channel count — the mailbox is depth-1 per channel, so concurrency *is* channel count |
| `PlanQueue` | bounded buffer of built-but-unissued operations. Its **depth** is the evidence the generator stayed ahead; reaching zero invalidates a live run (FR-062, FR-068) |
| `NodeTarget` | host plus local mailbox name |
| `NodeAgent` | per-node daemon. Holds no persistent state; started before a run, stopped after (FR-050) |
| `PayloadBuffer` | pre-filled reusable device buffer with an optional key stamp. Never per-operation bytes (FR-038) |
| `RunReport` | live: throughput, latency percentiles, plan-queue depth, lane utilisation, validity. Emit: completeness only, with the unmeasurable fields **absent rather than zero** (FR-071) |

## Cross-cutting invariants worth testing directly

| Invariant | Requirement | Why it needs its own test |
| --- | --- | --- |
| Plan is byte-identical across live/emit, batch sizes, lane counts | FR-072, SC-003 | The property a comment cannot enforce |
| Integral draws are uniform at the endpoints, not half-weighted | FR-009 | Silently wrong otherwise, and invisible in aggregate |
| Residual-life seeding produces flat churn from t=0 | FR-015 | The cohort artifact is periodic and slow to dephase |
| Keys identical across toolchains and machines | FR-029 | Remote hits depend on it |
| Nested-not-divergent chains for overlapping sets | FR-028 | The mechanism the whole reuse structure rests on |
| Emit report omits live-only fields | FR-071 | A zero is indistinguishable from a measurement |

# Data Model: Synthetic KV Workload Generator

**Phase 1 output** · Spec: `spec.md` · Contracts: `contracts/`

Concrete Rust representations of the spec's Key Entities. The normative field lists live in the
contracts; this file records *types, ownership, and the invariants that must be enforced in code*.

## Ownership map

| Concern | Crate | Why there |
| --- | --- | --- |
| Schema types, validation, distributions | `workload-model::schema`, `::dist` | Shared by every binary |
| Key derivation | `workload-model::keys` | Correctness-critical; one implementation |
| Corpus and session state | `workload-model::corpus`, `::session` | Generation is the model's job |
| Plan codec, manifest, digests | `workload-model::plan` | Written by the generator, read by three others |
| The four FR-056 statistics | `workload-model::stats` | **Must** be one implementation (FR-021i) |
| Trace containers | `workload-trace` | Keeps `arrow` out of the default build (SC-012) |

## Keys

```rust
/// An opaque KV block identity. Not an index; never ordered meaningfully.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(pub u64);

/// Who minted a key. Distinct from who reads it — the distinction is load-bearing.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct SessionId(pub u32);

/// Advances only when corpus churn is configured; 0 otherwise.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Generation(pub u32);
```

Three derivations, and they must not be collapsed into one function with flags — the namespaces are
the invariant:

```rust
fn trunk_child(parent: CacheKey, child_index: u32, gen: Generation) -> CacheKey;   // FR-008
fn private_child(parent: CacheKey, minter: SessionId, i: u32) -> CacheKey;         // FR-009c
fn root(root_index: u32, gen: Generation) -> CacheKey;                             // FR-009
```

**Invariants to enforce and test:**

- `Generation(0)` MUST reduce `trunk_child` to exactly `H(parent, child_index)` (FR-008), so the
  default is bit-identical to a build with no churn concept at all.
- `private_child` takes the **minting** session, never the reading one. An agent-fan-out child passes
  its *parent's* `SessionId` for the inherited prefix and its own below the spawn point (FR-009c).
  Getting this backwards turns every fan-out into a miss storm that reads as a cache result.
- Trunk and private namespaces MUST be disjoint by construction, not by probability (FR-007).
- Key identity MUST be computable from path alone — no arrival-order dependence, no global mutable
  state (FR-009b). This is what buys O(active paths) memory and independent per-node generation.

## Corpus

```rust
pub struct Corpus {
    block_bytes: Dist,            // a pure function of key identity (FR-011)
    roots: Roots,
    shared_depth: Dist,
    branching: BranchProfile,      // piecewise, NOT a scalar (FR-009e)
    branch_skew: f64,
    churn: Option<Churn>,          // None == immortal trunk (FR-016b default)
}

pub struct BranchProfile { segments: Vec<Segment> }   // sorted, non-empty, from_depth[0] == 0
pub struct Segment { from_depth: u32, fanout: f64, skew: Option<f64>, churn_half_life: Option<Duration> }
```

`BranchProfile` needs three methods and they are where the occupancy machinery lives:

```rust
fn fanout_at(&self, depth: u32) -> f64;
fn paths(&self, depth: u32, roots: u32) -> f64;      // roots * Π fanout(k), k in 1..=depth
fn occupancy(&self, depth: u32, sessions_per_window: f64, churn: Option<&Churn>) -> f64;
```

**Invariants:** every `fanout >= 1.0` (FR-009b) so the trunk is unbounded in depth; a non-integer
fanout is realised by randomised rounding to `floor`/`ceil` with `E[children] = m` **keyed on the
node**, not the visit (FR-009e), which is what keeps a long run reproducible while still stochastic;
`occupancy` MUST use the churn-adjusted window when churn is set (FR-016e) or the floor approves
sharing that churn then destroys.

## Sessions

```rust
pub struct Session {
    id: SessionId,
    root: CacheKey,               // drawn once, sticky for life (FR-009a)
    node: NodeIndex,              // sticky by default (FR-019a)
    mix_index: u8,
    turns_total: u16,
    turn: u16,                    // 1-based
    shared_depth: u32,
    private_depth: u32,
    lineage: Option<Lineage>,     // Some(..) for a spawned child
    live_descendants: u32,        // lineage-scoped lifetime (FR-018d)
}

pub struct Lineage { parent: SessionId, inherited_depth: u32, generation_of_tree: u8 }
```

**Path depth is computed, never stored per turn** (FR-014a):
`depth(turn N) = shared_depth + private_depth + Σ(i=2..N) growth_per_turn(i)`, with turn N's path a
strict prefix of turn N+1's — required by the rolling hash, since a changed prefix rehashes
everything below it.

**Lifecycle** (FR-014b/c): born on arrival → binds root and node → issues `turns_total` requests
separated by `think_time` → retired. Lifetime is `Σ think_time`, **derived, never a field**. Live
population is derived too — Little's law under `open_loop`, `arrival.concurrency` under
`closed_loop` (FR-015a) — and is the constant in FR-010's O(active paths) bound.

**`live_descendants` is a refcount, and it is correct here** for the reason it was wrong for the
shared trunk (FR-016c): within a lineage the children *are* the readers, the parent's context exists
for them, and the count is small and known. A parent's private keys are released only at
`live_descendants == 0` **and** the parent itself retired (FR-018d).

## Plan events

The wire layout is normative in `contracts/plan-format.md`: 40 bytes, every field naturally aligned,
record size a multiple of 8 so an array needs no packed intermediate.

```rust
#[repr(C)]
pub struct PlanEvent {
    t_ns: u64, key: u64, size: u32, request_id: u32,
    session_id: u32, depth: u32, turn: u16, node: u16,
    mix_index: u8, flags: u8, reserved: u16,
}
const _: () = assert!(size_of::<PlanEvent>() == 40);
```

**Flags**: bit 0 `REQUEST_START`, 1 `REQUEST_END`, 2 `WARMUP`, 3 `COLD`, 4–7 reserved and MUST be
zero. `COLD` states only that warmup did not pre-request the key — it is **not** a predicted miss,
and a consumer that hits on a `COLD` key has violated nothing.

**No `parent_session` field.** Lineage is recovered by prefix-matching, since a child's leading keys
*are* the parent's. Adding one would widen the record to 48 bytes for information already present,
and would be a second independently-writable statement of one fact — a plan could then assert a
lineage its keys contradict.

## Statistics

```rust
pub struct PlanStatistics {
    reuse_distance_cdf: Cdf,            // primary (FR-034a)
    compulsory_miss_floor: f64,         // miss rate at UNBOUNDED capacity — capacity-free
    sharing_depth_histogram: Histogram,
    request_length: Percentiles,
    unique_keys_over_time: Vec<(u64, u64)>,
    trunk_width_per_depth: Vec<(u32, u64)>,
    trunk_occupancy_per_depth: Vec<(u32, f64)>,
    working_set_bytes: u64,             // over run.wss_window, a REQUEST COUNT
}
```

Every field is capacity-free — a property of the reference stream, not of any cache. **No
Belady/OPT**: it evicts furthest-next-use *when full*, so its hit rate is a function of a capacity
the generator does not know (FR-034b).

The same code must compute these over a generated plan and over a real trace. `fit`, `report` and
`validate` all call it; none reimplements it (FR-021i).

## Trace records

`workload-trace` only. The invocation record is normative in `contracts/trace-io.md`.

**Normalise on ingest**: detect the population pattern once — delta versus full, by whether
`full_input_blocks` is empty — reconstruct full ordered block lists at the boundary, and let
everything downstream see one representation. Otherwise the branch leaks into every statistic that
walks a block list, and each such site is a fresh chance to get the trailing-partial-block convention
wrong: the delta form **excludes** the trailing partial block, the full form **includes** it.

```rust
pub enum Population { Delta, Full }
fn normalise(rec: &InvocationRecord, session: &SessionCtx, bs: u32) -> Vec<BlockId>;
```

## Validation

Rules 1–23 of `contracts/workload-schema.md` are a single pass over the parsed document, returning
**all** violations rather than the first — a half-configured document usually has several, and
reporting one at a time makes fixing it a guessing game.

Three deserve implementation attention because they are the ones that catch a document which is
internally consistent, passes everything else, and still measures the wrong thing:

- **Rule 16**, occupancy floor at `p99(shared_depth)`, churn-adjusted.
- **Rule 13**, a removed consumer-side key (`system:`, `topology.holder_tier`) must be rejected with
  a message naming design rule 6 and saying where the quantity went — these were documented schema in
  an earlier draft, so a stale document is a likely input rather than a typo.
- **FR-015b**, warmup shorter than the session-population ramp. A rejection, not a warning: the
  resulting numbers are wrong rather than merely noisy.

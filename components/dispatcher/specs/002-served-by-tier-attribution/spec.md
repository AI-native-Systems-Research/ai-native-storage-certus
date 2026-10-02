# Feature Specification: Serving-Tier Attribution (`served_by`)

**Feature Branch**: `002-served-by-tier-attribution`

**Created**: 2026-08-04

**Status**: Draft — reconciled 2026-09-25 against a Certus that changed under it; see
`## Reconciliation`.

**Input**: Expose, per looked-up key, **which tier actually served it** — local DRAM, local
SSD, a peer's DRAM, a peer's SSD — or why it was not served. Today a successful `Lookup`
is indistinguishable across all four, so no tiered hit rate is measurable.

**Why now — and as of 2026-09-28 the benefit is measured, not merely missing.** Phase 1's
counters shipped and gave the first reading: remote lookup serves **0.369%** of the keys
forwarded to a peer (2 443 of 661 534), contributing **0.255 percentage points** to a 31.3%
hit rate, while the same run declined **27.5%** of stores. Roughly 100:1 against, on this
workload. Details and caveats in `research.md` R6; the statement that there was "no evidence
remote lookup serves anything" described the instrument and is superseded.

Before that counter existed, two hardware measurements established remote lookup's **cost**
and neither could establish its **benefit**, because nothing could separate a local hit from
a remote one. Driving four instances that could
see each other versus the same workload with each instance isolated: hit rate 33.9% with
peers against 34.6% without — no detectable benefit — while store declines went from 41.2%
to 0.0%. A second sweep reproduced the decline half at a different rate and in a different
pacing mode (49.6% with peers, 1.5% without). So the feature the cluster most needs is not
faster remote lookup; it is an instrument that says whether remote lookup serves anything
at all. `certus_lookup_hits_total` counts blocks served without distinguishing the source,
and `certus_lookup_misses_total` read **0** against roughly 4M reserves.

## Scope and boundary

The datum this feature exposes already exists inside the dispatcher and is discarded at an
interface boundary. `IDispatchMap::lookup` returns exactly the discriminant needed —
`LookupResult::{NotExist, MismatchSize, BlockDevice, MemoryTier}`
(`components/interfaces/src/idispatch_map.rs:11-28`) — and both dispatchers match on it
throughout `batch_lookup`, but every arm collapses to `Ok(())` or `Err(DispatcherError)`
(`components/dispatcher/src/lib.rs:2106`,
`components/dispatcher-p2p/src/lib.rs:1584`). The tier is known one line before it is
thrown away.

**This is therefore an interface change, not only a transport change.** No new measurement
is introduced — the datum exists — but the surface is wider than it looks:
`IDispatcher::batch_lookup` returns `Vec<Result<(), DispatcherError>>`
(`components/interfaces/src/idispatcher.rs:360-363`), so the value has to be carried
through the `interfaces` crate before any server can report it.

In scope:

- A serving-tier taxonomy, and its representation in the `interfaces` crate.
- Carrying it out of `batch_lookup` in **both** `dispatcher` and `dispatcher-p2p`.
- Carrying the peer's advertised tier out of `remote-lookup` so remote hits split into
  peer-DRAM and peer-SSD.
- Exposing it on the **shm-queue control plane**, by widening the per-key `ok` byte that
  `LOOKUP` already returns (see *Boundary with the control plane*).
- Making the servers' aggregate hit/miss counters account for every request (see
  Clarifications), including a remote-lookup counter that does not exist today.
- Updating the **specs of every component this changes**, not only this one (see
  *Per-component documentation*).

Out of scope, deliberately: see `## Out of Scope`.

### Boundary with the control plane

**There is no gRPC surface any more.** It was removed by `97e26738` ("Remove gRPC; make
shm-queue the sole control transport"), which also deleted `apps/certus-server`'s spec
series. Every requirement in this spec that named a proto field or a gRPC method has been
rewritten against the mailbox; the earlier wording is recorded in `## Reconciliation`
rather than silently dropped, because a reader of the git history will find it.

The tier surfaces by **widening a byte that already exists**. `op_lookup` returns one `ok`
byte per key (`ok_flags`, `lib/shmq-dispatcher/src/translate.rs:567`), currently `0` or
`1`. `served_by` replaces that byte's value space, keeping `0` = not served so that any
reader testing `byte != 0` still sees a served key as served.

That is not an invention: `check_state` in the same wire module made exactly this move and
documented the rule — "Widened from a plain `bool`: `MISS`/`RESIDENT` keep the old `0`/`1`
meaning, `PENDING` is new, so a reader doing `byte != 0` still sees a pending key as
'exists'" (`lib/shmq-dispatcher/src/wire.rs:98-110`). This feature follows that precedent
for `LOOKUP`, which means **no new wire structure and no framing change**.

`CHECK` keeps `check_state` untouched. The two answer different questions: `check_state` is
about *residency* — is this key here, and is a store still in flight — while `served_by` is
about the *route* a served key travelled. A key can be `RESIDENT` to a check and served
from a peer to a lookup, and collapsing the two would lose exactly the distinction this
feature exists to expose.

This feature is filed under `components/dispatcher` because the dispatcher is where the
tier is resolved, and because this repo specifies `interfaces`-crate changes in the
*consuming* component's spec rather than under `components/interfaces/specs/` — the
precedent being `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:7`
(FR-001, which specifies the `IDispatcher` trait itself) with the trait surface mirrored as
`contracts/idispatcher.md`. The multi-unit reach is declared in the same form
`components/remote-lookup/specs/002-remote-lookup-rdma/spec.md:196` uses.

### Per-component documentation

Each Certus component carries its own complete specification, and those specifications feed
formal verification — `dispatcher-p2p`, `remote-lookup` and `remote-lookup-rdma-*` all
carry Creusot and Spin tooling. A change that edits a component's code without editing that
component's spec therefore does not merely leave documentation stale; it invalidates the
artifact a proof is written against.

So this feature is **not** done when `components/dispatcher` is updated. Every unit it
changes owns docs that must change with it:

| Unit changed | Spec that must be updated | Verification-bearing |
| --- | --- | --- |
| `components/interfaces` | `001-interfaces` — the taxonomy type and the `batch_lookup` signature | no |
| `components/dispatcher` | `001-dispatcher-cache-interface` and this spec | no |
| `components/dispatcher-p2p` | `001-gpudirect-cold-path` — its own `batch_lookup` arms | **yes** |
| `components/remote-lookup` | `002-remote-lookup-rdma` — the peer's advertised tier | **yes** |
| `lib/shmq-dispatcher` | no `specs/` dir; the wire change is documented in `wire.rs` beside `check_state` | no |

`apps/certus-server` and `apps/certus-server-yaml` own no specs — the former's were deleted
with gRPC — so their changes are covered by the units above.

### Relationship to the workload generator

**This is no longer a dependency in either direction, and the earlier wording that said it
was is withdrawn.** The workload generator was rewritten between this spec being written
and being picked up; its spec now mentions `served_by` and serving tiers **zero times**,
defines no `Outcome` entity, and its FR-039 — which this spec cited as assuming a
five-value taxonomy — now reads "The operation stream MUST be what the production client
would emit for the same workload". The generator was deliberately de-coupled from Certus's
internals, so it neither assumes a taxonomy nor needs updating to match one.

What survives is the *measurement* relationship, which is stronger than the spec
dependency was: the generator can drive a workload that exercises remote lookup
(`--probe lookup`, its FR-083) and can report a hit rate, but **cannot attribute it**. That
is the gap this feature closes, and it is why the motivation in `**Input**` is now a
measurement result rather than another spec's requirement.

One consequence of the original scoping has since been **removed rather than documented
around**: remote hits were to be attributed by the peer's *advertised* tier, splitting
`REMOTE_DRAM` from `REMOTE_SSD`. That split is withdrawn (2026-09-29) and remote hits carry a
single `REMOTE`. The reasoning is recorded in `contracts/served-by.md`; the short form is that
`REMOTE_SSD` named something it could not deliver — no byte ever crosses the fabric from a
peer's disk — while being an advertisement rather than an observation, decaying on repeat
access, at 0.81% of hits.

## Reconciliation (2026-09-25)

This spec was written on 2026-08-04 and picked up on 2026-09-25. Certus changed underneath
it in the interval, and three of its scoping decisions rested on things that no longer
exist. Recorded here rather than silently rewritten, because the git history shows the
earlier wording and a reader deserves to know which parts were re-verified.

**What was checked and still holds — the load-bearing premises are intact:**

| Premise | Status |
| --- | --- |
| `LookupResult` carries the four-way discriminant | holds, `idispatch_map.rs:11-28` |
| `IDispatcher::batch_lookup` discards it | holds, `idispatcher.rs:360-363` |
| Both dispatchers collapse every arm | holds, `dispatcher/src/lib.rs:2106`, `dispatcher-p2p/src/lib.rs:1584` |
| The peer's tier already crosses the wire | holds, `Avail::{None, Memory, Disk}`, `remote-lookup/src/wire.rs:23-30` |
| No remote-lookup wire change needed | holds |

Every line citation in the original had drifted and has been re-pinned.

**What changed, and what was done about it:**

1. **The gRPC surface is gone.** `97e26738` ("Remove gRPC; make shm-queue the sole control
   transport") removed it and deleted `apps/certus-server`'s spec series. This invalidated a
   boundary section, a P1 user story, a requirements block, a proto-artifacts block, four
   out-of-scope bullets and four assumptions. All are rewritten against the mailbox. The
   replacement is *smaller* than the original design: the tier widens the per-key `ok` byte
   `LOOKUP` already returns, following the precedent `check_state` set in the same module,
   so there is no new wire field and no framing change.
2. **The workload-generator dependency is withdrawn in both directions.** The generator was
   rewritten; its spec now mentions serving tiers **zero times**, defines no `Outcome`
   entity, and its FR-039 — cited here as assuming a five-value taxonomy — now says something
   else entirely. The instruction "that spec must be updated to match" was therefore acting
   on a document that had already moved. The motivation is now a measurement result instead:
   two hardware runs established remote lookup's cost and neither could establish its
   benefit.
3. **Per-component documentation was not a requirement and now is.** The original declared
   multi-unit reach but committed only to its own spec. Each Certus component carries a
   complete specification that its formal verification is written against, and two of the
   units this feature changes — `dispatcher-p2p` and `remote-lookup` — carry Creusot and Spin
   tooling. FR-030..FR-032 and *Per-component documentation* make the obligation explicit.

**Not re-verified, and flagged rather than assumed:** the implementor and call-site counts in
*Dependencies on other components* predate both the gRPC removal and the upstream dispatcher
rework. They must be re-counted at plan time; the line numbers there are known stale.

## Clarifications

### Session 2026-08-04 (initial design — resolved)

- Q: Is the serving tier for a remote hit knowable, and does it need a wire-protocol change?
  → A: **Knowable today, no wire change.** The peer's tier already crosses the wire as
  `Avail::{None, Memory, Disk}` in KEY_RESPONSE (`components/remote-lookup/src/wire.rs:21-49`,
  `:103-108`) and is retained requester-side per peer for the whole operation
  (`components/remote-lookup/src/operation.rs:53-56`). It dies in three places, none of them
  the protocol: `Operation::results()` projects `KeyState` only (`operation.rs:149-160`);
  `KeyState` has no tier dimension (`operation.rs:19-27`); and `Avail` is not exported from
  the crate into `interfaces`. **This feature surfaces the advertised tier only.**
- Q: Then can `Phase1`/`Phase2` be used as the DRAM/SSD proxy? → A: **No, and it must not
  be.** The two are correlated but not equivalent, in four independent ways.
  `on_key_response` has no phase check, so a peer-DRAM hit can finalize with
  `phase == Phase2` (`components/remote-lookup/src/actor.rs:405-424`). `try_retry` tries
  Memory *then* Disk with no phase gate, so a disk fetch can occur while `phase == Phase1`
  (`actor.rs:622-623`, called unguarded from `:530`). `Phase` is stored **per operation, not
  per key** (`operation.rs:76`), so a mixed batch carries one value for both. And it
  transitions on quorum/timeout, never on a tier event (`actor.rs:698-705`). Any
  implementation that reads phase instead of `Avail` is wrong.
- Q: What does `REMOTE_SSD` mean, given the transport? → A: **It was asked, answered, and the
  answer is why the value no longer exists.** It could only ever have meant "the responding peer
  had to read from its SSD in order to serve this," never "the NIC read from SSD": the RDMA read
  is always out of the responder's DRAM, because a disk-tier key is promoted into the peer's
  memory tier *before* the write (`components/remote-lookup/src/server.rs:243-265`) and the
  initiator sources bytes only via `IMemoryTier::peek`
  (`components/remote-lookup-rdma-initiator/src/lib.rs:157-169`). It was also a *first-touch*
  property — having served it, the peer holds it in DRAM, so the next request advertises
  `Memory`. A value whose name overstates it, which is an advertisement rather than an
  observation, and whose population decays, was not worth a wire slot at 0.81% of hits.
  **Collapsed to `REMOTE` on 2026-09-29**; the peer's disk work is now counted on the peer,
  where it is a stable aggregate. Full reasoning in `contracts/served-by.md`.
- Q: How is a size mismatch classified? → A: **Its own bucket, `SIZE_MISMATCH`.** It is
  neither a hit (no data delivered) nor a plain miss (the key *is* present). Giving it a
  distinct value changes no dispatcher behaviour: today `LookupResult::MismatchSize` yields
  `InvalidParameter` (`components/dispatcher/src/lib.rs:2093-2098`) and such keys are never
  offered to the remote path, because the remote block selects only `KeyNotFound`
  (`lib.rs:2469-2476`). Folding it into `MISS` would have required changing that behaviour;
  this decision deliberately does not.
- Q: Must every request land in exactly one bucket? → A: **Yes, and that forces a seventh
  value, `ERROR`.** Today `hits + misses ≠ requests`: `lookup_misses` increments only on
  `KeyNotFound` (`apps/certus-server-yaml/src/service.rs:437-439`) and every other error
  falls through to `:441` counted as neither. A lookup that was attempted and failed (e.g. a
  failed batched `stream_synchronize`) is not a hit and not a miss, so a complete taxonomy
  needs a bucket for it. `ERROR` is deliberately flat — it does not record which tier was
  being attempted (see `## Out of Scope`).
- Q: Does `served_by` describe the route taken or where the entry ends up? → A: **The route
  taken.** A local SSD hit in `dispatcher` is promoted into DRAM as part of being served
  (`lib.rs:2128-2137`), so "served from SSD" and "now DRAM-resident" are both true of the
  same request; the former is what a hit-rate measurement means. This distinction is sharper
  in `dispatcher-p2p`, where the cold path does **not** populate DRAM synchronously (FR-014).
- Q: Which value does a zero byte mean? → A: **`0` means "not served", and nothing else.**
  *Amended 2026-09-25.* This originally reserved `0` for `SERVED_BY_UNSPECIFIED` because
  proto3 requires a zero default and it let a client detect an old server. With gRPC gone the
  constraint is inverted: `LOOKUP`'s byte already means "not served" at `0`, so reserving it
  for "unknown" would silently reclassify every miss. There is therefore **no
  `UNSPECIFIED` value** — an old server is detected by the mailbox's own protocol version,
  not by an in-band sentinel (`check_state` set this precedent too: it widened into the
  unused values above `1` and left `0` alone).

### Dependencies on other components (implied by the above)

1. **`components/interfaces` — the sole `interfaces`-crate change for this feature.** Adds a
   public `ServedBy` enum and changes `IDispatcher::batch_lookup`'s return type
   (`idispatcher.rs:360-363`). Lands as its own commit, ahead of the implementations, and
   updates `components/interfaces/specs/001-interfaces` with it. Blast radius is bounded and
   compiler-enforced, but **every implementor and call site count below predates the gRPC
   removal and the upstream dispatcher rework and MUST be re-counted at plan time** — the
   line numbers in particular are known stale.
2. **`components/dispatcher`** — attribution at every resolution site in `batch_lookup`
   (FR-008..FR-013). This is the component that owns the feature.
3. **`components/dispatcher-p2p`** — the same, plus the cold-path residency difference in
   FR-014. It must not be left a generation behind, which is the failure mode this
   component has hit before.
4. **`components/remote-lookup`** — `IRemoteLookup::batch_lookup` must carry the peer's
   advertised `Avail` out (FR-016), which requires `KeyState` or the result projection to
   gain a tier dimension (`operation.rs:19-27`, `:149-160`) and an `interfaces`-visible type.
   **No wire-protocol change.**
5. **`lib/shmq-dispatcher`** — widens `LOOKUP`'s per-key `ok` byte (`translate.rs:567`,
   `wire.rs:98-110`). It owns no `specs/` directory, so the wire change is documented beside
   `check_state`, which already documents its own widening and its compatibility rule.
6. **`apps/certus-server` and `apps/certus-server-yaml`** — the counter correction
   (FR-024..FR-026), including a remote-lookup counter. Neither server may report a tier it
   did not receive from the dispatcher. Neither owns a `specs/` directory:
   `apps/certus-server`'s was deleted with gRPC in `97e26738`.
7. **`components/dispatch-map`, `components/memory-tier`, `components/eviction-policy-lru`** —
   no change. The discriminant already exists at `idispatch_map.rs:11-28`.
8. **The Python connector** (`certus-shmq-connector`) — reads the `LOOKUP` byte, so the
   widening reaches it. It keeps working untouched by the compatibility rule (`byte != 0`),
   and capturing the tier there is optional and separately scoped.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Per-Key Serving Tier Through the Interface (Priority: P1)

A component author calls `IDispatcher::batch_lookup` with a batch of keys spanning DRAM
residency, SSD residency, and absent keys, and receives — per key, in input order — both the
success/failure result and the tier that served it.

**Why this priority**: Every other story reads this value. It is also the only story that can
be tested without a server, a GPU, or a fabric, and it is where the taxonomy's invariants are
enforceable in one place.

**Independent Test**: Fully testable in the two dispatchers' existing unit suites, in
staging mode, with no hardware: populate a mixed batch, force known residency, and assert the
per-key tier.

**Acceptance Scenarios**:

1. **Given** a batch containing one DRAM-resident key, one SSD-resident key, and one absent
   key, **When** `batch_lookup` is called, **Then** the results are attributed `DRAM`, `SSD`,
   and `MISS` respectively, in input order.
2. **Given** a key present at a different size than requested, **When** `batch_lookup` is
   called, **Then** it is attributed `SIZE_MISMATCH` and not `MISS`.
3. **Given** a batch in which the batched GPU synchronization fails, **When** `batch_lookup`
   returns, **Then** every key whose copy was in flight is attributed `ERROR` and none is
   reported as a hit.
4. **Given** any batch, **When** `batch_lookup` returns, **Then** every element carries
   exactly one attribution and none is `UNSPECIFIED`.
5. **Given** a batch of N keys, **When** `batch_lookup` returns, **Then** the result length
   is N and attribution index i corresponds to input index i.

---

### User Story 2 - Tiered Hit Rate Over the Control Plane (Priority: P1)

A client issues `LOOKUP` over the shm-queue mailbox and reads, per key, which tier served
it — so that a tiered hit rate and per-tier latency percentiles become computable from the
response alone.

**Why this priority**: it is what makes the value observable outside the process, and it is
the half that turns the measurement gap in `**Input**` from unanswerable into answerable.
It delivers on a single node.

**Independent Test**: Run a single-node server with a working set exceeding the memory tier,
issue lookups, and confirm the DRAM/SSD split in the response bytes moves as
`--memory-tier-size` is varied.

**Acceptance Scenarios**:

1. **Given** a key served from local DRAM, **When** the client reads that key's `LOOKUP`
   response byte, **Then** it is the DRAM value and is non-zero.
2. **Given** a `LOOKUP` for an absent key, **When** the client reads its byte, **Then** it is
   `0` — preserving today's "not served" meaning exactly.
3. **Given** an existing reader that tests `byte != 0` to mean "served", **When** it reads a
   response from a server built with this feature, **Then** it classifies every key as it did
   before the widening. This is the compatibility rule `check_state` already set.
4. **Given** a client that knows the widened value space, **When** it reads any served key,
   **Then** the byte names a specific tier and never a catch-all "unspecified" value — an
   attributed lookup that cannot say where it came from is the defect this feature exists to
   remove, not an acceptable state.
5. **Given** the same batch issued to a server with peers and to one isolated, **When** the
   bytes are compared, **Then** the difference is attributable to the remote values alone —
   which is the check the two hardware measurements in `**Input**` could not make.

---

### User Story 3 - Remote Hits Split by Peer Tier (Priority: P2)

An engineer measuring cross-node behaviour sees remote hits separated into "the peer had it
in DRAM" and "the peer had to read its SSD", so that the Phase-1 and Phase-2 paths of remote
lookup become separately measurable.

**Why this priority**: It is the measurement only Certus needs (workload-generator US4), but
it depends on US1 and needs a multi-node cluster, so it follows the single-node stories.

**Independent Test**: In the existing `remote-lookup` mesh tests, hold a key on a peer in
DRAM versus flushed to the peer's SSD, and assert the attribution differs accordingly —
no RDMA hardware required for the mocked mesh path.

**Acceptance Scenarios**:

1. **Given** a key held in a peer's memory tier, **When** it is fetched remotely, **Then**
   the requester attributes it `REMOTE`.
2. **Given** a key held only on a peer's SSD, **When** it is fetched remotely, **Then** the
   requester also attributes it `REMOTE` — identically, and on every fetch, because the
   requester does not distinguish the peer's tier — **and** the responding peer counts one
   peer-triggered promotion on the first fetch and none on the second.
4. **Given** a remote fetch deduplicated by single-flight such that this caller is a
   follower, **When** it is attributed, **Then** it carries the same tier as the leading
   fetch and never `UNSPECIFIED`.
5. **Given** a key no peer holds, **When** the remote lookup fails, **Then** it is
   attributed `MISS` and not `ERROR`.

---

### User Story 4 - Complete and Reconcilable Accounting (Priority: P2)

An operator reading the server's aggregate counters finds that hits, misses, and other
outcomes sum to the number of entries requested, and that the aggregate agrees with the
per-entry attribution in the same responses.

**Why this priority**: Without it the new per-key field would contradict the existing
counters, and a disagreement between two numbers the server itself publishes is worse than
having only one. Fixing it is small once US1 lands.

**Independent Test**: Issue a batch containing a hit, a miss, and a forced failure; scrape
`/metrics`; assert the three counters sum to the batch size and match the responses.

**Acceptance Scenarios**:

1. **Given** a batch containing successes, misses, and non-miss failures, **When** the
   counters are read, **Then** hits + misses + errors equals the number of entries requested.
2. **Given** any completed `Lookup`, **When** the aggregate counters and the per-entry
   `served_by` values are compared, **Then** they agree.
3. **Given** a server with no client ever calling `TakeEvents`, **When** lookups are served,
   **Then** lookup attribution is unaffected — it does not depend on any drain.

---

### User Story 5 - Attribution Under Both Dispatchers (Priority: P2)

An engineer runs the same measurement against a `full` profile and a `full-p2p` profile and
gets attribution with the same meaning from both, with the p2p cold path's different
residency behaviour documented rather than silently divergent.

**Why this priority**: `dispatcher-p2p` is selected by a build-time profile and is the target
of the GPUDirect work; attribution that only worked under one dispatcher would silently
mis-describe the other. P2 because it duplicates US1's mechanism rather than adding a new one.

**Independent Test**: Run the two dispatchers' unit suites over the same fixture batches and
assert identical attribution for identical residency, except where FR-014 specifies otherwise.

**Acceptance Scenarios**:

1. **Given** identical residency, **When** the same batch is looked up under each dispatcher,
   **Then** both attribute the same tier.
2. **Given** `dispatcher-p2p`'s SSD-to-GPU cold path, **When** a key is served, **Then** it
   is attributed `SSD` even though DRAM was not populated synchronously.
3. **Given** a key served by p2p's cold path and requested again before the asynchronous DRAM
   backfill completes, **When** it is attributed, **Then** `SSD` is permitted and correct.
4. **Given** a multi-region (N>1) request under `dispatcher-p2p`, **When** it is rejected,
   **Then** it is attributed `ERROR` and not `MISS`.

---

### Edge Cases

- **A key that misses locally and then hits remotely.** The local pass records `MISS` and the
  remote pass overwrites it. Attribution MUST reflect the final resolution, and MUST NOT
  count the key as both a miss and a remote hit.
- **A result overwritten after attribution.** A failed batched sync rewrites already-`Ok`
  results to `IoError` (`components/dispatcher/src/lib.rs:2588-2590`) and the
  concurrent-promotion recovery pass rewrites `AlreadyExists` (`:2615-2620`). Attribution
  MUST be rewritten in lockstep; an attribution left behind by an overwritten result reports
  a tier for a key that failed.
- **Concurrent promotion.** A key recovered by `serve_concurrently_promoted` was served out
  of DRAM after waiting for another thread's promotion; it is `DRAM`, not `SSD`.
- **Cold-load staging.** An entry served through a staging buffer because the tier was
  saturated was still read from SSD, and is `SSD`.
- **Single-flight follower with no landing slot.** A deduplicated follower owns no
  `LandingSlot` (`components/remote-lookup/src/actor.rs:541-554`), so its tier must come from
  the leading operation's record rather than its own.
- **`AlreadyExists` publish path.** A key marked satisfied on another operation's publish
  (`actor.rs:576-582`) has a recorded peer that did not fill DRAM; the tier must not be taken
  from that peer's advertisement without checking.
- **A peer that advertises `Avail::None`.** It is not a holder; it contributes no tier and
  must not be attributed.
- **An empty batch.** Returns an empty result vector; no attribution, no counter movement.
- **A batch whose keys span two tiers and one fails.** Per-key attribution must remain
  independent; one `ERROR` must not contaminate its neighbours.

## Requirements *(mandatory)*

### Outcome taxonomy

- **FR-001** *(revised 2026-09-29 — the two remote values collapsed to one; see
  `contracts/served-by.md`)*: The system MUST define a serving-tier taxonomy with exactly six
  meaningful values: `DRAM`, `SSD`, `REMOTE`, `MISS`, `SIZE_MISMATCH`, and `ERROR`. It MUST NOT
  subdivide `REMOTE` by the serving peer's tier.
- **FR-002**: Every looked-up key MUST be attributed exactly one value. There MUST NOT be an
  "unknown" or "other" outcome.
- **FR-003**: The taxonomy MUST distinguish *hits* (`DRAM`, `SSD`, `REMOTE`), in which data
  was delivered to the caller's destination, from *non-hits* (`MISS`, `SIZE_MISMATCH`,
  `ERROR`), in which it was not.
- **FR-004**: Attribution MUST describe the route by which the request was served, not the
  entry's residency after serving.
- **FR-005**: `MISS` MUST mean the key was not found in any tier, local or remote.
- **FR-006**: `SIZE_MISMATCH` MUST mean the key was present but at a different size than
  requested, and MUST NOT be reported as `MISS`.
- **FR-007**: `ERROR` MUST mean the lookup was attempted and failed for a reason other than
  absence or size mismatch.

### Dispatcher attribution

- **FR-008**: `components/dispatcher` MUST attribute every key at each resolution site in
  `batch_lookup`: the warm DRAM hit, the cold SSD promotion (all of its sub-paths — pooled,
  inline fallback, no-drives, and staging), the remote-delivery block, the
  concurrent-promotion recovery pass, and the not-found and size-mismatch arms.
- **FR-009**: When a result is overwritten after attribution, the attribution MUST be
  overwritten with it.
- **FR-010**: A key served out of DRAM after waiting for a concurrent promotion MUST be
  attributed `DRAM`.
- **FR-011**: A key served through a cold-load staging buffer MUST be attributed `SSD`.
- **FR-012**: A key that missed locally and was then served remotely MUST be attributed with
  its remote tier only, and MUST NOT also be counted as a local miss.
- **FR-013**: Attribution MUST NOT change the outcome, latency, or ordering of any lookup;
  it MUST NOT introduce a lock, an allocation on the per-key path, or an additional
  `IDispatchMap` call.

### `dispatcher-p2p` attribution

- **FR-014**: `components/dispatcher-p2p` MUST attribute its SSD-to-GPU cold path as `SSD`
  even though that path does not populate DRAM synchronously, and the specification of this
  behaviour MUST record that a subsequent request for the same key may legitimately be
  attributed `SSD` again until the asynchronous DRAM backfill completes.
- **FR-015**: `dispatcher-p2p` MUST attribute a rejected multi-region (N>1) request as
  `ERROR`.

### Remote attribution

- **FR-016** *(WITHDRAWN 2026-09-29, replaced)*: previously required `components/remote-lookup`
  to carry the responding peer's advertised tier out of `batch_lookup`. With `REMOTE` collapsed
  there is no consumer, and `IRemoteLookup::batch_lookup` keeps its present signature. A remote
  hit is attributed from the per-key success of the remote pass, which the dispatcher already
  has. **Replaced by**: `components/remote-lookup` MUST count the keys it promoted from its own
  disk in order to serve a *peer's* request, and expose that count — the question the split was
  reaching for, answered as an aggregate about pressure this node's peers place on it. Specified
  in `components/remote-lookup/specs/002-remote-lookup-rdma`.
- **FR-017** *(revised 2026-09-29 with FR-016)*: Remote attribution MUST be derived from the
  per-key outcome of the remote pass — the key was served by a peer, or it was not. It MUST NOT
  be derived from the operation's phase, and it MUST NOT be derived from the peer's advertised
  availability either, because no tier is carried any more. **The phase prohibition is kept
  even though nothing now needs a tier**, because phase is the cheap proxy anyone
  reconstructing this would reach for first: phase is per operation rather than per key, a
  peer-DRAM hit can finalize in Phase 2, a disk fetch can occur in Phase 1, and it transitions
  on quorum and timeout rather than on any tier event.
- **FR-018**: This feature MUST NOT change the remote-lookup wire protocol, MUST NOT change
  `WIRE_VERSION`, and MUST remain interoperable with an unmodified peer.

### Control-plane surface

*Rewritten 2026-09-25: this section specified a proto field on a gRPC surface that
`97e26738` removed. The requirements below carry the same intent onto the mailbox.*

- **FR-019**: The taxonomy MUST be expressed by **widening the existing per-key `ok` byte**
  that `LOOKUP` returns (`lib/shmq-dispatcher/src/translate.rs:567`). The feature MUST NOT
  add a wire field, change framing, or alter the request encoding.
- **FR-020**: The byte value `0` MUST continue to mean **not served**. There MUST NOT be an
  "unspecified" value: reserving `0` for unknown would silently reclassify every miss, and an
  old server is detected by the protocol version rather than by an in-band sentinel.
- **FR-021**: A conforming server MUST NOT emit a value outside the taxonomy on any `LOOKUP`
  response, and MUST NOT emit a catch-all "served, tier unknown" value — that state is the
  defect this feature removes.
- **FR-022**: The widening MUST be backward compatible in the sense `check_state` already
  established (`lib/shmq-dispatcher/src/wire.rs:98-110`): a reader testing `byte != 0`
  MUST classify every key exactly as it did before. The Python connector MUST keep working
  untouched.
- **FR-023**: A server MUST NOT report a tier it did not receive from the dispatcher; it MUST
  NOT infer one from latency, error code, or any other proxy.
- **FR-023a**: `CHECK` MUST keep `check_state` unchanged. `check_state` answers *residency*
  and `served_by` answers the *route* a served key travelled; a key may be `RESIDENT` to a
  check and peer-served to a lookup, so collapsing the two would lose the distinction this
  feature exists to expose.
### Server counters

- **FR-024**: The servers' aggregate lookup counters MUST account for every requested entry,
  such that hits plus misses plus errors equals the number of entries requested.
- **FR-025**: The aggregate counters MUST agree with the per-entry attribution in the same
  responses.
- **FR-026**: Lookup attribution MUST NOT depend on any client draining the eviction event
  stream.

### Verification

- **FR-027**: Each attribution value MUST have a test that fails if that value is
  mis-assigned, in both dispatchers.
- **FR-028**: The mocks used to test attribution MUST be verified to model residency
  faithfully enough that the assertions are not vacuous, and the tests MUST be demonstrated
  to fail against deliberately wrong attribution before being trusted.
- **FR-029**: The test suites MUST cover both the default and the `integrity-check` feature
  configurations.

### Per-component documentation

*Added 2026-09-25, replacing a section about generated proto bindings that no longer exist.*

- **FR-033** *(New 2026-10-02)*: A clean eviction that fails MUST record **which** of its two
  causes applied, as separate counters on `TierEventStats`: `evictions_blocked_by_pin` when
  `IDispatchMap::try_evict_to_block` returns `ActiveReferences`, and
  `evictions_blocked_unpersisted` when it refuses for want of an `ssd_offset`. The call site MUST
  match the error variant; testing `is_ok()` and incrementing a single "eviction failed" counter
  does NOT satisfy this.
  - **Rationale.** The two causes mean opposite things and are indistinguishable at the call site.
    A held read pin implicates a *reader* — an in-flight load, or a peer being served over RDMA.
    A missing `ssd_offset` means write-through has not landed, which is expected under write load
    and self-correcting. `reserve_memory`'s own comment names both. A combined count can support
    neither conclusion, and a store-decline measurement of 72% was unattributable for exactly
    this reason.
  - Both counters MUST be recorded per *candidate examined*, so one scan past several pinned
    entries increments several times.
  - `evictions_blocked_unpersisted` MUST be counted, not inferred by subtracting
    `evictions_blocked_by_pin` from the scanned total. Subtraction silently absorbs any third
    cause into whichever counter was not measured; counting both makes their sum against the
    scanned total a check that the two explanations are exhaustive.
- **FR-034** *(New 2026-10-02)*: `TierEventStats` MUST carry `eviction_scans_exhausted`, counting
  clean-eviction scans that examined every candidate and freed none. This is the event that
  becomes a declined store — `reserve_memory` retries it against the `--store-backpressure-ms`
  budget and drops the store when the budget elapses — so it MUST be recorded whichever cause
  blocked the scan.
- **FR-035** *(New 2026-10-02)*: `dispatcher-p2p` does **not** report FR-033/FR-034 and its
  values for them are to be read as *unmeasured*, not as zero. That component has eviction paths
  which can be refused by a held pin, but they are free functions holding no counter handle, so
  reporting them is a threading change to a component this work cannot exercise (p2p needs its own
  profile and a loaded `gdrdrv`). The gap MUST stay declared in both the component's code and this
  spec for as long as it exists.
  - This is distinguished deliberately from the route-counter defect recorded in SC-008: reporting
    hits with `lookup_hits_dram == 0` was **self-contradictory** and therefore a defect, whereas an
    unmeasured counter is a gap — acceptable only while declared. Adding fields to `TierEventStats`
    is not compiler-enforced, so neither case is caught by a build.

- **FR-030**: Every component whose code this feature changes MUST have its **own** spec
  updated in the same change: `components/interfaces` (`001-interfaces`),
  `components/dispatcher` (`001-dispatcher-cache-interface` and this spec),
  `components/dispatcher-p2p` (`001-gpudirect-cold-path`) and `components/remote-lookup`
  (`002-remote-lookup-rdma`). A component whose code moves while its spec does not is not
  merely undocumented: each component's specification is the artifact its formal
  verification is written against, and `dispatcher-p2p` and `remote-lookup` both carry
  Creusot and Spin tooling.
- **FR-031**: Where a changed unit owns no `specs/` directory — `lib/shmq-dispatcher`, both
  servers — the change MUST be documented at the site instead, beside the definition it
  modifies. For the wire byte that means documenting the widening next to `check_state`,
  which already documents its own.
- **FR-032**: This spec MUST NOT be the only record of the taxonomy. The value space MUST be
  defined once, in `components/interfaces`, and every other document MUST reference it rather
  than restate it — a taxonomy written down twice is a taxonomy that will disagree with
  itself, which is the failure this feature's own history demonstrates.

### Key Entities

- **ServedBy** — the serving-tier taxonomy of FR-001. One value per looked-up key.
- **LookupOutcome** — the pair of (attribution, result) returned per key by
  `IDispatcher::batch_lookup`, replacing the bare `Result<(), DispatcherError>`.
- ~~**RemoteTier**~~ — withdrawn 2026-09-29 with FR-016. No tier crosses
  `IRemoteLookup::batch_lookup`; `REMOTE` is derived from per-key success alone.

## Requirement coverage after Phase 1 (2026-09-28)

Recorded so a reader need not infer it from the task list. Phase 1 changed only `dispatcher`,
`interfaces` (two struct fields), `shmq-dispatcher` and `certus-server-yaml`.

| Requirement | State after Phase 1 |
|---|---|
| FR-024 | **Satisfied for the server-side tally.** Every requested entry now lands in exactly one of hits / misses / errors: `KeyNotFound` → miss, every other dispatcher error → error, and entries held back before dispatch (their GPU handle would not open) → error, counted as `requested - dispatched`. Held via `Translator::tally_lookup`, tested as the identity itself, and shown to fail against both former holes. |
| FR-025 | **Not addressed.** There is no per-entry attribution yet to agree with — that is the `ServedBy` value of Phase 3. The aggregates are internally consistent, which is a weaker claim. |
| FR-026 | **Satisfied incidentally, and it needs re-checking in Phase 3.** The counters live in `TierEventCounters` and are read by polling `tier_event_stats()`, which does not touch the eviction event stream at all. The risk this requirement guards against returns if attribution is ever routed through that stream. |
| FR-027, FR-028 | **Not applicable yet** (no attribution values). Their *method* was applied anyway: every Phase 1 test was mutation-verified against the defect it guards. |
| FR-029 | **Not addressed.** Phase 1's tests were not run under `integrity-check`. |

Two Phase 1 findings that bear on requirements elsewhere in this spec:

- **FR-030's "compiler-enforced" claim is false for a struct field.** Adding fields to
  `TierEventStats` compiled with zero errors because the other implementors build it with
  `::default()`. A future implementor can therefore silently under-report. See
  `contracts/idispatcher.md`, which asserts the opposite and must be corrected in Phase 2.
- **The two accounting holes never fired in the measured workload** (nothing was held back, no
  non-`KeyNotFound` error occurred), so FR-024's closure is hardening rather than a correction
  of any recorded number. The hardware result in `research.md` R6 is not evidence about them.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For any `Lookup` batch against a conforming server, every entry carries exactly
  one serving-tier attribution and none is unspecified.
- **SC-002**: A single-node capacity sweep shows the reported DRAM-to-SSD served ratio moving
  monotonically as DRAM capacity is reduced against a fixed working set — the attribution
  responds to the thing it claims to measure.
- **SC-003** *(revised 2026-09-29)*: On a multi-node cluster, a key held only by a peer —
  whether in that peer's DRAM or on its disk — is reported `REMOTE`, and a locally served key
  never is. Separately, a key held only on a peer's disk increments that peer's peer-triggered
  promotion count on first fetch, and does not increment it on the second.
- **SC-004**: Hits plus misses plus errors equals entries requested, for every batch,
  including batches containing non-miss failures.
- **SC-005**: The same batch against `dispatcher` and `dispatcher-p2p` yields identical
  attribution for identical residency, except as FR-014 permits.
- **SC-006**: An unmodified client continues to work against a new server, and a new client
  against an unmodified server reports "attribution unsupported" rather than a tier.
- **SC-007**: A remote lookup against an unmodified peer still succeeds, demonstrating no
  wire-protocol change.
- **SC-008**: Measured throughput and per-key latency on the remote-lookup benchmark are
  statistically indistinguishable from the pre-change baseline at n ≥ 8 per side — the
  attribution is free.
- **SC-009**: Every taxonomy value has a test that has been observed to fail when that value
  is deliberately mis-assigned.

## Out of Scope

- **Recording which tier an `ERROR` was attempted from.** `ERROR` is flat. Adding an
  attempted-tier dimension is a refinement, not a prerequisite for hit-rate measurement.
- **Serve-time ground truth for remote hits.** The responder computes the real tier and
  discards it (`components/remote-lookup/src/server.rs:216-238`, destroyed at `:249-263` and
  `:268-287`). Capturing it requires a new wire message type — appending a field to
  RDMA_STATUS is unsafe, because the codec frames by record count with no length prefix or
  spare field, so an old decoder mis-aligns silently — and there is no capability
  negotiation to gate it on. Deferred to its own feature.
- **Which peer served a remote hit.** Peer identity is available
  (`components/remote-lookup/src/operation.rs:42-49`) but is a separate concern from tier,
  and is unavailable for single-flight followers without further work.
- **Making `GetIoStats` usable as a cross-check under `p2p-native`.** The counters are zeroed
  unless the `rw-telemetry` feature is enabled, and
  `apps/certus-server-yaml/Cargo.toml:53` forwards that feature only to `dispatcher`, not to
  `dispatcher-p2p` — so no feature combination enables them under `--features p2p-native`.
  Recorded as a known limitation for the workload-generator spec's SC-007, which depends on
  it.
- **Re-deriving the workload generator's 5% `GetIoStats` agreement tolerance.** `GetIoStats`
  is device-level and drive-aggregated (`components/dispatcher/src/lib.rs:3256-3271`) and
  includes background promotion traffic, so exact agreement with critical-path SSD bytes was
  never achievable. Belongs to that spec.
- **Splitting `certus_evictions_total` into demoted versus removed**, and its dependence on a
  client calling `TakeEvents` (`apps/certus-server-yaml/src/service.rs:861`). Adjacent
  accounting defects, not lookup attribution.
- **Per-tier latency histograms or OTel attributes.** This feature makes per-tier latency
  *computable by a client* from the response. Exporting it as server-side metrics with a
  `tier` attribute is observability work belonging to the `certus-server` OTel series.
- **Reconciling with the SimPy simulator's three-way `hot`/`cold`/`miss` bucketing**
  (`tools/simulator/certus_sim/metrics.py:19-20`), which is modelled rather than measured.
- **Anything to do with the removed gRPC surface.** Four bullets here previously scoped out
  proto artifacts — a reduced proto in `apps/baseline-generalized-fs`, drift in three sets of
  checked-in Python stubs, a frozen spec-contract proto copy, and the absence of a proto lint
  in CI. `97e26738` removed gRPC and made shm-queue the sole control transport, so none of
  those artifacts is on this feature's path. They are left here as a record of what the
  earlier scoping worried about, not as work.
- **Capturing the tier in the Python connector.** The widened byte reaches
  `certus-shmq-connector`, which keeps working untouched by FR-022's compatibility rule.
  Reading and recording the tier there is useful and separately scoped — it is what would let
  a vLLM run report a tiered hit rate — but it is not needed for the measurement in
  `**Input**`, which the node agent can make directly.
- **Any change to the eviction policy, dispatch-map, or memory-tier interfaces.**

## Assumptions

- The tier that resolves a lookup is already known internally at each resolution site, so
  this feature adds no measurement — only propagation. Verified against
  `components/interfaces/src/idispatch_map.rs:9-28` and both dispatchers' `batch_lookup`.
- **No byte ever crosses the fabric from a peer's disk.** The RDMA read is always from the
  peer's DRAM, after the responder promotes any disk-resident key. This is why the taxonomy
  carries no remote-SSD value; the peer's disk work is counted on the peer instead.
- **Remote attribution is the peer's advertisement, not serve-time truth.** The peer
  re-resolves at serve time and the entry may have been promoted, evicted, or demoted in
  between (`components/remote-lookup/src/server.rs:216-238`), so a small fraction of remote
  attributions can be wrong in a way this feature does not detect. This is accepted as
  precise enough for aggregate hit-rate measurement and inadequate for per-request forensics.
- **The peer-triggered promotion count is a first-touch quantity, and that is now a feature
  rather than a defect.** Serving from disk promotes the entry into the peer's DRAM, so the
  same key counts once and not again. As a *rate* on the responder this is exactly the signal
  wanted — how much cold work peers are causing — whereas as a per-key attribution on the
  requester it was a decaying fraction that no fixed configuration could hold steady.
- The dispatcher selection remains a build-time profile choice (`CERTUS_PROFILE`), so both
  dispatchers must be verified separately rather than switched at runtime. This is why
  FR-014's cold-path difference cannot be tested by flipping a flag.
- `CacheKey` remains an opaque `u64`, and the batched `LOOKUP` over the shm-queue mailbox
  remains the measurement path.
- **The compiler enforces the interface change; it does not enforce the wire change.**
  Widening `IDispatcher::batch_lookup`'s return type makes every implementor and call site a
  compile error until it is updated, so none can be missed. The `ok` byte is a `u8`: writing
  the wrong value there compiles cleanly. That asymmetry is why FR-028 requires the
  attribution tests be demonstrated to fail against deliberately wrong attribution before
  being trusted — a mock that returns a plausible tier for the wrong reason would pass a test
  that only checks the byte is non-zero.
- Widening a byte whose `0` meaning is preserved is backward compatible for every reader that
  tests `byte != 0`, which is how the connector reads it. A reader that instead matched
  exhaustively on `0`/`1` would break — none is known, and `check_state`'s precedent
  established the same assumption once already.

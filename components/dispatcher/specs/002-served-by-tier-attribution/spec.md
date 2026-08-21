# Feature Specification: Serving-Tier Attribution (`served_by`)

**Feature Branch**: `synthetic-workload-generation` — renamed 2026-08-21 from
`002-served-by-tier-attribution`, which is what this feature's branch was called before the synthetic
workload generator grew to be almost all of its content. This design still rides on that branch; only
the branch was renamed, and **this feature directory keeps its own name** because a feature directory
is not a branch.

**Created**: 2026-08-04

**Status**: Draft

**Input**: Expose, per looked-up key, **which tier actually served it** — local DRAM, local
SSD, a peer's DRAM, a peer's SSD — or why it was not served. Today a successful `Lookup`
is indistinguishable across all four, so no tiered hit rate is measurable. Raised as the
blocking prerequisite of
`apps/workload-generator/specs/001-synthetic-workload-generator` (spec.md:93-98, 107-110),
which cannot measure hit rate per tier (its US3, US4) without it.

## Scope and boundary

The datum this feature exposes already exists inside the dispatcher and is discarded at an
interface boundary. `IDispatchMap::lookup` returns exactly the discriminant needed —
`LookupResult::{NotExist, MismatchSize, BlockDevice, MemoryTier}`
(`components/interfaces/src/idispatch_map.rs:9-28`) — and both dispatchers match on it
throughout `batch_lookup`, but every arm collapses to `Ok(())` or `Err(DispatcherError)`
(`components/dispatcher/src/lib.rs:2088-2143`,
`components/dispatcher-p2p/src/lib.rs:1631-1701`). The tier is known one line before it is
thrown away.

**This is therefore an interface change, not only a proto change.** The prerequisite note
in the workload-generator spec (spec.md:671) calls it "a plumbing change rather than new
bookkeeping"; that is accurate about the *bookkeeping* — no new measurement is introduced —
but it understates the surface. `IDispatcher::batch_lookup` returns
`Vec<Result<(), DispatcherError>>` (`components/interfaces/src/idispatcher.rs:377-380`), so
the value has to be carried through the `interfaces` crate before any server can report it.

In scope:

- A serving-tier taxonomy, and its representation in the `interfaces` crate.
- Carrying it out of `batch_lookup` in **both** `dispatcher` and `dispatcher-p2p`.
- Carrying the peer's advertised tier out of `remote-lookup` so remote hits split into
  peer-DRAM and peer-SSD.
- Exposing it on the gRPC surface of **both** `apps/certus-server` and
  `apps/certus-server-yaml`.
- Making the servers' aggregate hit/miss counters account for every request (see
  Clarifications).

Out of scope, deliberately: see `## Out of Scope`.

### Boundary with the servers' gRPC surface

`apps/certus-server-yaml` has no `specs/` directory and no `.specify/` tree, so it cannot
own a feature. `apps/certus-server` has its own spec series (001-003) covering the gRPC
server, operational config, and OTel observability. This feature is filed under
`components/dispatcher` because the dispatcher is where the tier is resolved and because
this repo specifies `interfaces`-crate changes in the *consuming* component's spec rather
than under `components/interfaces/specs/` — the precedent being
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:250` (FR-001, which
specifies the `IDispatcher` trait itself) with the trait surface mirrored as
`contracts/idispatcher.md`. The multi-unit reach is declared below in the same form
`components/remote-lookup/specs/002-remote-lookup-rdma/spec.md:97` uses.

The proto field is a *presentation* of the dispatcher's datum. Both servers' protos must
change identically; they are byte-identical today except for two comments.

### Relationship to the workload generator

This feature is scoped to what the workload generator needs and nothing more. Two
consequences of that scoping are visible below and are intentional:

- The taxonomy is **seven-valued, not five** (`## Clarifications`). The workload-generator
  spec assumes five (`spec.md:502`, FR-039; US3 acceptance 1; the `Outcome` entity at
  `spec.md:579`). That spec must be updated to match; the mismatch is recorded here rather
  than silently reconciled.
- Remote hits are attributed by the peer's **advertised** tier, which is precise enough for
  a hit-rate measurement but is not serve-time ground truth. The distinction is spelled out
  in FR-016..FR-018 and in `## Assumptions`, because a report that says `REMOTE_SSD` is
  making a weaker claim than it appears to.

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
- Q: What does `REMOTE_SSD` mean, given the transport? → A: **"The responding peer had to
  read from its SSD in order to serve this," never "the NIC read from SSD."** The RDMA read
  is always out of the responder's DRAM: a disk-tier key is promoted into the peer's memory
  tier *before* the write (`components/remote-lookup/src/server.rs:243-265`) and the
  initiator sources bytes only via `IMemoryTier::peek`
  (`components/remote-lookup-rdma-initiator/src/lib.rs:157-169`). This is also why
  `REMOTE_SSD` is a property of a *first* touch: having served it, the peer now holds it in
  DRAM, so a second request for the same key advertises `Memory`.
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
- Q: Which value does a proto3 default of 0 mean? → A: `SERVED_BY_UNSPECIFIED = 0` MUST
  exist, because proto3 requires it and because it is the only way a client can detect an
  old server, but a conforming server MUST never emit it (FR-020, FR-021).

### Dependencies on other components (implied by the above)

1. **`components/interfaces` — the sole `interfaces`-crate change for this feature.** Adds a
   public `ServedBy` enum and changes `IDispatcher::batch_lookup`'s return type
   (`idispatcher.rs:377-380`). Lands as its own commit, ahead of the implementations. Blast
   radius is bounded and compiler-enforced: 4 implementors
   (`components/dispatcher/src/lib.rs:1631`, `components/dispatcher-p2p/src/lib.rs:1121`,
   `components/remote-lookup/src/seams.rs:629` which is `unimplemented!`,
   `apps/certus-server/src/service.rs:1093` test mock), 2 production call sites
   (`apps/certus-server-yaml/src/service.rs:420`, `apps/certus-server/src/service.rs:441`),
   and ~14 test call sites across the two dispatchers.
2. **`components/dispatcher`** — attribution at every resolution site in `batch_lookup`
   (FR-008..FR-013). This is the component that owns the feature.
3. **`components/dispatcher-p2p`** — the same, plus the cold-path residency difference in
   FR-014. It must not be left a generation behind, which is the failure mode this
   component has hit before.
4. **`components/remote-lookup`** — `IRemoteLookup::batch_lookup` must carry the peer's
   advertised `Avail` out (FR-016), which requires `KeyState` or the result projection to
   gain a tier dimension (`operation.rs:19-27`, `:149-160`) and an `interfaces`-visible type.
   **No wire-protocol change.**
5. **`apps/certus-server` and `apps/certus-server-yaml`** — identical proto addition
   (FR-019..FR-021) and the counter correction (FR-024..FR-026). Neither server may report a
   tier it did not receive from the dispatcher.
6. **`components/dispatch-map`, `components/memory-tier`, `components/eviction-policy-lru`** —
   no change. The discriminant already exists at `idispatch_map.rs:9-28`.
7. **Generated gRPC bindings** — the four Rust `tonic_build` consumers regenerate at build
   time and need no action beyond recompiling (note that `apps/remote-lookup-bench/build.rs`
   compiles the `certus-server-yaml` copy, so the benchmark picks the field up
   automatically). Python is different: three sets of **checked-in** stubs are produced by
   hand-run `generate_pb.sh` scripts, and regenerating them is a deliberate step (FR-030).

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

### User Story 2 - Tiered Hit Rate Over gRPC (Priority: P1)

A benchmark client issues `Lookup` against a server and reads, per entry, which tier served
it — so that a tiered hit rate and per-tier latency percentiles become computable from the
response alone.

**Why this priority**: This is the story the workload generator is blocked on (its US3), and
it is what makes the value observable outside the process. It delivers on a single node.

**Independent Test**: Run a single-node server with a working set exceeding DRAM capacity,
issue lookups, and confirm the DRAM/SSD split in the responses moves as capacity is varied.

**Acceptance Scenarios**:

1. **Given** a successful `Lookup` served from local DRAM, **When** the client reads the
   `EntryResult`, **Then** `served_by` is `SERVED_BY_DRAM`.
2. **Given** a `Lookup` for an absent key, **When** the client reads the `EntryResult`,
   **Then** `success` is false, `error_code` is `ERROR_CODE_KEY_NOT_FOUND`, and `served_by`
   is `SERVED_BY_MISS`.
3. **Given** any `Lookup` response from a conforming server, **When** every entry is
   examined, **Then** no entry carries `SERVED_BY_UNSPECIFIED`.
4. **Given** a client built against the new proto talking to an **old** server, **When** it
   reads `served_by`, **Then** it observes `SERVED_BY_UNSPECIFIED` and can report the server
   as not supporting attribution rather than mis-reporting a tier.
5. **Given** both `apps/certus-server` and `apps/certus-server-yaml`, **When** the same batch
   is issued to each, **Then** both report attribution with identical semantics.

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
   the requester attributes it `REMOTE_DRAM`.
2. **Given** a key held only on a peer's SSD, **When** it is fetched remotely for the first
   time, **Then** the requester attributes it `REMOTE_SSD`.
3. **Given** the same SSD-held key is fetched remotely a second time, **When** it is
   attributed, **Then** `REMOTE_DRAM` is permitted and correct, because serving it promoted
   it into the peer's DRAM.
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

- **FR-001**: The system MUST define a serving-tier taxonomy with exactly seven meaningful
  values: `DRAM`, `SSD`, `REMOTE_DRAM`, `REMOTE_SSD`, `MISS`, `SIZE_MISMATCH`, and `ERROR`.
- **FR-002**: Every looked-up key MUST be attributed exactly one value. There MUST NOT be an
  "unknown" or "other" outcome.
- **FR-003**: The taxonomy MUST distinguish *hits* (`DRAM`, `SSD`, `REMOTE_DRAM`,
  `REMOTE_SSD`), in which data was delivered to the caller's destination, from *non-hits*
  (`MISS`, `SIZE_MISMATCH`, `ERROR`), in which it was not.
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

- **FR-016**: `components/remote-lookup` MUST carry the responding peer's advertised tier out
  of `batch_lookup`, per key, so that a remote hit resolves to `REMOTE_DRAM` or
  `REMOTE_SSD`.
- **FR-017**: Remote attribution MUST be derived from the peer's advertised availability, and
  MUST NOT be derived from the operation's phase.
- **FR-018**: This feature MUST NOT change the remote-lookup wire protocol, MUST NOT change
  `WIRE_VERSION`, and MUST remain interoperable with an unmodified peer.

### gRPC surface

- **FR-019**: Both servers' `EntryResult` MUST carry a `served_by` field expressing the
  taxonomy, added identically to `apps/certus-server/proto/dispatcher.proto` and
  `apps/certus-server-yaml/proto/dispatcher.proto`.
- **FR-020**: The field MUST reserve a zero value meaning "unspecified", so that a new client
  can detect an old server.
- **FR-021**: A conforming server MUST NOT emit the unspecified value on any `Lookup`
  response.
- **FR-022**: The addition MUST be wire-compatible: an old client MUST continue to work
  against a new server without modification.
- **FR-023**: A server MUST NOT report a tier it did not receive from the dispatcher; it MUST
  NOT infer one from latency, error code, or any other proxy.
- **FR-023a**: `served_by` MUST be populated on `Lookup` responses. Its meaning on the other
  nine RPCs that reuse `EntryResult` MUST be specified explicitly — either populated or
  documented as unspecified-by-design — so that no consumer has to guess.
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

### Generated bindings and other proto artifacts

- **FR-030**: The three sets of checked-in Python stubs MUST either be regenerated as an
  explicit, separately reviewable step, or be left untouched with their staleness recorded.
  Regeneration MUST NOT silently import the unrelated drift two of them already carry.
- **FR-031**: The reduced proto copy in `apps/baseline-generalized-fs/proto/` and the frozen
  spec-contract copy under `apps/certus-server/specs/001-grpc-dispatcher-server/contracts/`
  MUST be explicitly decided about — changed, or left with the divergence recorded — rather
  than overlooked because they share the `certus.dispatcher.v1` package name.

### Key Entities

- **ServedBy** — the serving-tier taxonomy of FR-001. One value per looked-up key.
- **LookupOutcome** — the pair of (attribution, result) returned per key by
  `IDispatcher::batch_lookup`, replacing the bare `Result<(), DispatcherError>`.
- **RemoteTier** — the peer's advertised availability as carried out of
  `IRemoteLookup::batch_lookup`, from which `REMOTE_DRAM`/`REMOTE_SSD` is derived.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For any `Lookup` batch against a conforming server, every entry carries exactly
  one serving-tier attribution and none is unspecified.
- **SC-002**: A single-node capacity sweep shows the reported DRAM-to-SSD served ratio moving
  monotonically as DRAM capacity is reduced against a fixed working set — the attribution
  responds to the thing it claims to measure.
- **SC-003**: On a multi-node cluster, a key held only in a peer's DRAM is reported
  `REMOTE_DRAM` and a key held only on a peer's SSD is reported `REMOTE_SSD` on first fetch.
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
- **Attribution in `apps/baseline-generalized-fs`.** It is a baseline comparison app with its
  own deliberately reduced proto (7 RPCs, no `ipc_handles`) that happens to share the
  `certus.dispatcher.v1` package name. It has four exhaustive `EntryResult` literals of its
  own and breaks only if its copy is edited. Leaving it unattributed is the default; FR-031
  requires that be a decision rather than an oversight.
- **Fixing the pre-existing drift in the checked-in Python stubs.** Two of the three sets are
  already behind the live proto on unrelated messages — one lacks `LookupEntry.ipc_handles`
  and `ReserveEntry.session_id`; one is frozen at a five-RPC surface. That drift predates this
  feature and repairing it belongs to its own change (FR-030).
- **Refreshing the frozen spec-contract proto copy** under
  `apps/certus-server/specs/001-grpc-dispatcher-server/contracts/dispatcher.proto`, which is a
  documentation artifact nothing compiles.
- **Adding a proto lint or compatibility gate to CI.** None exists — no `buf`, no
  `protolock`, no proto reference in the Jenkinsfile or the GitHub workflows. Worth having,
  but not a prerequisite for this field.
- **Any change to the eviction policy, dispatch-map, or memory-tier interfaces.**

## Assumptions

- The tier that resolves a lookup is already known internally at each resolution site, so
  this feature adds no measurement — only propagation. Verified against
  `components/interfaces/src/idispatch_map.rs:9-28` and both dispatchers' `batch_lookup`.
- **`REMOTE_SSD` means the peer read its SSD to serve the request, not that data moved off
  SSD over the fabric.** The RDMA read is always from the peer's DRAM. A report consuming
  this field is making that weaker claim.
- **Remote attribution is the peer's advertisement, not serve-time truth.** The peer
  re-resolves at serve time and the entry may have been promoted, evicted, or demoted in
  between (`components/remote-lookup/src/server.rs:216-238`), so a small fraction of remote
  attributions can be wrong in a way this feature does not detect. This is accepted as
  precise enough for aggregate hit-rate measurement and inadequate for per-request forensics.
- **`REMOTE_SSD` is a transient, first-touch property.** Because serving from disk promotes
  the entry into the peer's DRAM, a repeated remote fetch of the same key is expected to
  report `REMOTE_DRAM`. A test or report that expects a stable `REMOTE_SSD` fraction from a
  fixed holder configuration is mis-specified.
- Both dispatcher protos remain byte-identical in the fields they define, so one field
  addition applies to both without divergence.
- The dispatcher selection remains a build-time profile choice (`CERTUS_PROFILE`), so both
  dispatchers must be verified separately rather than switched at runtime.
- `CacheKey` remains an opaque `u64` and the existing batched gRPC lookup surface remains the
  measurement path.
- **The Rust side of this change is compiler-enforced and the Python side is not.** Both
  servers build `EntryResult` through exhaustive struct literals with no defaulting
  initializer, so the four literals that must gain the field cannot be missed. The
  checked-in Python stubs carry no such guarantee, and no CI gate checks proto compatibility,
  so those steps rest on review.
- Adding a field and an enum to proto3 is backward- and forward-compatible, and every known
  client reads `EntryResult` by field access rather than by exhaustive match or full-struct
  equality — so no existing consumer breaks by construction.

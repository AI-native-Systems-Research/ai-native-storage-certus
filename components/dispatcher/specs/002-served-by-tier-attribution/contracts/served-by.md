# Contract: Serving-Tier Taxonomy and Its Control-Plane Surface

**Version**: 1
**Status**: Draft
**Producers**: `components/dispatcher`, `components/dispatcher-p2p` (via `components/remote-lookup` for the two remote values)
**Consumers**: `apps/certus-server-yaml`'s counters, and any `LOOKUP` client

This is the normative reference for what each attribution value means and how it crosses the
shm-queue control plane. The Rust-side interface delta is `contracts/idispatcher.md`.

## The taxonomy

Six meaningful values. Every looked-up key gets exactly one.

| Value | Data delivered? | Meaning |
| --- | --- | --- |
| `DRAM` | yes | Local memory tier hit; the block was already resident. |
| `SSD` | yes | A local data drive was read to serve this request. |
| `REMOTE` | yes | A peer served it. **Not** subdivided by the peer's tier — see below. |
| `MISS` | no | Not found in any tier, local or remote. |
| `SIZE_MISMATCH` | no | Present, but at a different size than requested. |
| `ERROR` | no | Attempted and failed for some other reason. |

Two properties of this taxonomy are load-bearing and easy to get wrong:

1. **It describes the route, not the residency.** In `dispatcher`, an SSD hit is promoted
   into DRAM as part of being served, so "served from SSD" and "now in DRAM" are both true of
   one request. `SSD` is the honest answer because it is what the request cost.
2. **`REMOTE` is deliberately not split by the peer's tier**, and the reasoning matters
   because splitting it is the obvious thing to want. See the next section.

### Why `REMOTE` is one value and not `REMOTE_DRAM` / `REMOTE_SSD` (decided 2026-09-29)

An earlier revision of this contract split them, on the peer's advertised tier carried out of
`IRemoteLookup::batch_lookup`. **That split is withdrawn.** It is a real design that looked
right, so the reasons are recorded here rather than deleted — do not reinstate it without
answering all four:

1. **`REMOTE_SSD` would not have described where the bytes came from.** The responder
   *promotes disk-resident keys into its own memory tier before the RDMA read*
   (`components/remote-lookup/src/server.rs`, `promote_to_memory_tier`, batched once per
   request). There is no RDMA-from-SSD path in this system, so every remotely served byte
   leaves a peer's DRAM. `REMOTE_SSD` could only ever have meant "a peer's disk read was on
   this request's critical path" — a true and useful statement, but not the one its name makes.
2. **It would have been an advertisement, not an observation.** The value came from what the
   peer announced, which may be stale by the time the key is served. That was a deliberate
   choice (serve-time ground truth needs a new message type, see below), but it means the
   value is one step removed from what happened.
3. **It decays, so no fixed configuration yields a stable fraction.** Serving from disk leaves
   the entry in the peer's DRAM, so the *next* fetch of that key advertises memory. The split
   fraction therefore falls monotonically in a static cluster, and any test or report
   expecting it to hold still is mis-specified. That defect was already recorded against the
   split; collapsing removes it rather than documenting around it.
4. **The volume cannot carry the complexity.** Remote lookup serves **0.369%** of what it is
   asked and **0.81%** of all hits (measured 2026-09-28, feature Phase 1). `REMOTE_SSD` is a
   first-touch subset of that, further narrowed because peer selection prefers memory-resident
   peers (`actor.rs`, `find(Avail::Memory).or_else(|| find(Avail::Disk))`). It would have been
   the least legible of the wire values at the smallest volume.

**The question the split was reaching for is still worth answering, and is answered better
elsewhere**: "are peers doing SSD reads on our behalf?" is a question about *pressure we cause
on peers*, which is an aggregate, not a per-key property of our own lookups. It is answered by
a responder-side counter of peer-triggered promotions — see
`components/remote-lookup/specs/002-remote-lookup-rdma` — which needs no wire value, no
per-key plumbing, and no `IRemoteLookup` tier delta.

### Hits versus non-hits

`DRAM`, `SSD`, `REMOTE` are hits. `MISS`, `SIZE_MISMATCH`, `ERROR` are not.
A hit is reported if and only if the lookup succeeded, so a consumer can compute an object
hit rate as `hits / total` without needing to know the error taxonomy.

### Why `SIZE_MISMATCH` is not folded into `MISS`

The cache *has* the key; it does not have it at the requested size. Reporting `MISS` would
conflate "you must populate this from scratch" with "your size model disagrees with what is
stored", which are different problems for a caller. Keeping it separate also means this
feature changes no dispatcher behaviour: a size mismatch currently yields
`InvalidParameter`, and because the remote-delivery pass selects only `KeyNotFound`, such
keys are never offered to peers. Folding it into `MISS` would have made size-mismatched keys
remote-eligible — a behaviour change smuggled in under an attribution feature.

### Why `ERROR` exists

A lookup that was attempted and failed is neither a hit nor a miss. Without a bucket for it,
"every request is attributed" is false, and the aggregate counters cannot be made to sum to
the request count — which is precisely today's defect: the server increments `lookup_misses`
only for `KEY_NOT_FOUND` and counts every other failure as neither.

`ERROR` is deliberately flat. It does not record which tier was being attempted when the
failure occurred. That refinement is deferred; it is not needed to measure hit rate.

## Control-plane surface

*Rewritten 2026-09-25. This section specified a proto3 enum and an `EntryResult` field on a
gRPC surface that `97e26738` removed. The intent is preserved; the mechanism is smaller.*

### The one channel available

`op_lookup` returns exactly one byte per key — `ok_flags`, currently `0` or `1`
(`lib/shmq-dispatcher/src/translate.rs:567`). It is the **only** per-key channel in a
`LOOKUP` response. So attribution either widens that byte or changes the framing, and
widening is available because `check_state` in the same module already did it and documented
the rule: `MISS`/`RESIDENT` keep their `0`/`1` meaning and `PENDING` takes `2`, so a reader
testing `byte != 0` is unaffected (`lib/shmq-dispatcher/src/wire.rs:98-110`).

### **DECISION — needs sign-off:** the wire carries a 5-value projection, not all 7

The taxonomy has six values, but only three of them mean *served*. On this byte `0` already
means *not served*, and `MISS`, `SIZE_MISMATCH` and `ERROR` are all not-served — so they
cannot each take a distinct non-zero value without making `byte != 0` report "served" for a
key whose data was never delivered. That would be worse than losing a distinction: a reader
would act on absent data.

So the byte carries **not-served, or which tier served**:

```text
0  not served          (was: ok = 0 — meaning preserved exactly)
1  DRAM                local memory tier hit; already resident
2  SSD                 a local data drive was read to serve this request
3  REMOTE              a peer served it
```

and the full six-value `ServedBy` lives in `components/interfaces`, where the dispatcher
produces it and the **server's counters consume it**. That is where the not-served breakdown
is actually needed: FR-024 requires hits plus misses plus errors to equal entries requested,
which is a counter reconciliation, not a per-key wire question. `MISS`, `SIZE_MISMATCH` and
`ERROR` therefore remain distinguishable in the place that reconciles them and collapse to
`0` on the wire, where the client's only question is "was this key served, and if so from
where".

**Why this is not a loss.** A tiered hit rate needs the numerator split by tier and the
denominator counted — the byte gives the first, the counters give the second. If a client
ever needs the per-key not-served reason it must come with a framing change, and that is a
separate feature rather than something to smuggle into a byte whose zero value is load
bearing.

**What would change this decision:** evidence that a client needs per-key
`SIZE_MISMATCH` versus `MISS` discrimination. The known consumer — the vLLM connector —
treats a size mismatch as a miss by design (`size-mismatch = cache miss`), so it does not.

### Compatibility

- **`0` keeps its meaning exactly**, so every reader testing `byte != 0` classifies every key
  as it did before. The Python connector reads it that way and needs no change.
- **There is no "unspecified" value, deliberately.** The proto3 design needed one because
  proto3 reserves zero as a default and it doubled as version detection. Here `0` is already
  spoken for, and reserving it for "unknown" would silently reclassify every miss as an
  unattributed hit. Version detection belongs to the mailbox's own protocol version.
- **A conforming server never emits a value outside `0..=3`.** No wire enforcement, so it
  needs a test. Values `4..=255` are unassigned; a reader MUST treat an unknown non-zero
  value as *served, tier unknown* rather than as not-served, so that a future split of
  `REMOTE` cannot turn a hit into a miss in an old reader.
- **The interface change is compiler-enforced; the byte is not.** Widening
  `IDispatcher::batch_lookup`'s return type makes every implementor and call site a compile
  error until updated. Writing the wrong `u8` compiles cleanly, which is why the attribution
  tests must be shown to fail against deliberately wrong attribution before being trusted.

### Scope on other operations

`CHECK` keeps `check_state` unchanged. The two answer different questions — `check_state` is
*residency* ("is it here, is a store in flight"), `served_by` is *route* ("where did this
served key come from") — and a key can be `RESIDENT` to a check and peer-served to a lookup.
Collapsing them would destroy the distinction this feature exists to expose.

No other operation gains attribution. Stating that explicitly is the point: a value that is
sometimes meaningful depending on which operation produced it is the ambiguity that gets
discovered by a wrong dashboard six months later.

## The `IRemoteLookup` delta — WITHDRAWN (2026-09-29)

**No `IRemoteLookup` change is required by this feature.** This section previously specified
carrying the peer's advertised tier out of `batch_lookup` so that `REMOTE_DRAM` and
`REMOTE_SSD` could be told apart. Collapsing them to a single `REMOTE` removes the only
consumer, and `batch_lookup` keeps its present signature:

```rust
fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>>;
```

`REMOTE` needs nothing new. The dispatcher already learns, per key, whether remote lookup
succeeded — it zips `remote_results` back against the batch — and that is exactly the
question `REMOTE` answers. The internal paths that made the tier version awkward (a
single-flight follower owning no landing slot, the `AlreadyExists` publish path having a
recorded peer that did not fill DRAM) are moot for the same reason: all of them still produce
a per-key success, and success is all `REMOTE` asserts.

### Findings kept, because they outlive the withdrawn delta

- **Phase is not a tier proxy, and must never be used as one.** Phase 1 / Phase 2 and tier are
  correlated but not equivalent: a peer-DRAM hit can finalize in Phase 2, a disk fetch can
  occur in Phase 1, phase is stored per operation rather than per key, and it transitions on
  quorum and timeout rather than on any tier event. This is recorded as a standing caution for
  anyone tempted to reconstruct a tier cheaply.
- **The peer wire protocol cannot absorb a new field, only a new message.** The codec frames
  by record count with **no length prefix and no spare or reserved field**, so appending a byte
  to an existing message mis-aligns an old decoder from the second record onward and fails
  **silently rather than detectably**. There is no capability negotiation to gate a change on,
  and bumping `WIRE_VERSION` makes old peers drop every frame as unknown. Any future
  serve-time ground-truth feature must therefore take the shape of a *new message type*.
  `WIRE_VERSION` stays at 1 and unmodified peers stay interoperable.
- **The tier information itself still exists**, should a future feature want it: it arrives as
  `Avail::{None, Memory, Disk}` in the peer's KEY_RESPONSE and is retained per peer for the
  operation, then discarded at three points — the result projection reads key state only, key
  state has no tier dimension, and `Avail` is not exported into `interfaces`. It is used today
  for peer *selection* (memory-resident peers are preferred), not for reporting.

## Internal-to-wire mapping

**Not one-to-one, and that is the decision above.** The six internal values project onto
four wire values, because three of them mean *not served* and the byte's `0` already says so.
A server MUST NOT infer a tier from a latency or any other proxy; it may only report what the
dispatcher returned, and it MUST NOT invent a wire value for a not-served key.

| `ServedBy` (Rust, in `interfaces`) | `LOOKUP` byte | Served? | Counter it increments |
| --- | --- | --- | --- |
| `Dram` | `1` | yes | `lookup_hits`, `lookup_hits_dram` |
| `Ssd` | `2` | yes | `lookup_hits`, `lookup_hits_ssd` |
| `Remote` | `3` | yes | `lookup_hits`, `remote_lookup_hits` |
| `Miss` | `0` | no | `lookup_misses` |
| `SizeMismatch` | `0` | no | `lookup_misses` (see below) |
| `Error` | `0` | no | `lookup_errors` |

The three `0` rows are why the internal taxonomy stays six-valued: the counters need the
distinction that the wire cannot carry, and FR-024's reconciliation — hits plus misses plus
errors equals entries requested — is only checkable if `Error` is counted apart from `Miss`.

`SizeMismatch` counting as a miss matches the known consumer: the vLLM connector treats a
size mismatch as a cache miss by design. Whether it deserves its own counter is left open;
what it must not do is silently vanish from the reconciliation.

## Counter reconciliation

The servers' aggregate lookup counters must become consistent with the per-entry field, which
means correcting an existing defect rather than adding to it. Today `lookup_hits` counts
successes and `lookup_misses` counts only `KEY_NOT_FOUND`; every other failure is counted as
neither, so the two do not sum to the number of entries requested.

Required:

- Hits, misses, and errors MUST sum to entries requested, for every batch.
- The aggregate counters MUST agree with the `served_by` values in the same responses.
- Attribution MUST NOT depend on a client draining the eviction event stream. (The existing
  `certus_evictions_total` does have that dependency — it only advances inside the
  `TakeEvents` handler — which is a separate defect, noted and out of scope.)

Whether `SIZE_MISMATCH` is counted under errors or gets its own counter is an implementation
choice for `plan.md`; the invariant is that the sum is complete either way.

Splitting the aggregate counters *by tier* is explicitly not required here. This feature makes
a tiered hit rate computable by a client from the responses; exporting it as server-side
per-tier metrics is observability work belonging to the `certus-server` OTel series, which
today attaches only `op` and `drive` attributes and has no `tier` dimension anywhere.

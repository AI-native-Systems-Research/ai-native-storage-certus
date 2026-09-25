# Phase 0 Research: Serving-Tier Attribution

**Feature**: `002-served-by-tier-attribution` | **Date**: 2026-09-25

Every finding below was checked against the code on `certus-lookup-observability` at
`origin/unstable` `fa7a9d74`. Where a claim is not yet settled it says so, with the
experiment that would settle it.

## R1. Is a remote-hit counter independent of the attribution work?

**Yes, and this is what makes a counter-first phase possible.**

The obvious reading is that counting remote hits needs `served_by`, because the server is
what exposes counters and the server only learns the tier if the dispatcher returns it. That
reading is wrong: **the counter does not have to be incremented by the server.**

**Corrected 2026-09-25**: an earlier draft of this section said three sites. There are three
`self.remote_lookup.get()` guards, but only one is a lookup — `:1986` is `join_cluster` and
`:2070` is `leave_cluster`. The fetch path is **`components/dispatcher/src/lib.rs:2575`
alone**, which is simpler than the draft claimed.

There, the dispatcher collects the `KeyNotFound` indices, calls `rl.batch_lookup`, and zips
the answers back — so it knows per key whether the peer served it, one line before it folds
the answer into `Result<(), DispatcherError>`. A counter incremented there needs:

- no change to `IDispatcher::batch_lookup`'s signature,
- no change to the `interfaces` crate,
- no change to the shm-queue wire,
- no change to `dispatcher-p2p`, `remote-lookup`, or either server's request path.

So the counter is a **strictly smaller change** than attribution, deliverable and testable
on its own, and it answers the question that currently has no instrument at all.

**Consequence for phasing**: the counter is Phase 1 and does not block on, or pre-empt, the
`ServedBy` taxonomy. When attribution lands, the counter's source moves from an internal
observation to the taxonomy — a refactor, not a redesign, and the counter's *meaning* does
not change.

## R2. Why does `certus_lookup_misses_total` read 0?

Measured on the cluster: roughly 4M reserves, a generator-computed hit rate of ~34%, and
`certus_lookup_misses_total` reading **0**. Since the generator saw ~66% of keys not served,
the counter should be large. Three candidate causes were investigated; two are confirmed
defects, the third is open.

### Confirmed: entries held back before the batch are never counted

`op_lookup` resolves each entry's regions first, and entries whose handles fail to open are
**held back from the batch** and reported as `ok = 0`
(`lib/shmq-dispatcher/src/translate.rs:571-583`). The counting loop runs only over the
entries that *were* dispatched, so a held-back entry increments neither `hits` nor `misses`.

It is reported to the client as not-served and is invisible to the counters. That is exactly
the accounting hole FR-024 exists to close: hits plus misses does not equal entries
requested.

### Confirmed: every error except `KeyNotFound` is silently dropped

```rust
// Only KeyNotFound counts as a miss; other errors (e.g. transient
// I/O) mirror the gRPC service, which excludes them from misses.
Err(interfaces::DispatcherError::KeyNotFound(_)) => misses += 1,
Err(_) => {}
```

`translate.rs:604-605`. The comment is honest about the intent — it mirrors the behaviour of
the gRPC service that has since been removed — but `Err(_) => {}` means an entry that failed
for any other reason is counted as neither hit nor miss. Again FR-024.

Note this is *not* the cause of a zero reading on its own, because `NotExist` **does** map to
`KeyNotFound` (`components/dispatcher/src/lib.rs`, the `LookupResult::NotExist` arm), so an
ordinarily-absent key should be counted.

### CONFIRMED, and it is the cause: a remote miss is relabelled as an I/O error

This began as a hypothesis about the remote path and is now settled **by reading the code**,
not by inference.

At `components/dispatcher/src/lib.rs:2624-2627`, when the peer does not have the key:

```rust
if let Err(e) = remote_res {
    results[pos] =
        Some(Err(DispatcherError::IoError(format!("remote lookup: {e}"))));
    continue;
}
```

Every remote failure — including a plain peer miss — overwrites the original `KeyNotFound`
with **`IoError`**. `translate.rs`'s `Err(_) => {}` then drops it. So with peers configured,
a locally-absent key is forwarded to the peer, comes back absent, and is counted as neither
a hit nor a miss. That is the zero.

**It is worse than an accounting hole: it is a misclassification.** A key that no node holds
is a *miss*, not an I/O error. Any consumer reasoning about error rates sees transport
failures that never happened.

**And the fix is small, because the distinction already exists.**
`IRemoteLookup::batch_lookup` returns `Vec<Result<(), RemoteLookupError>>` with
`RemoteLookupError::{NotFound, TransportError(String)}`
(`components/interfaces/src/iremote_lookup.rs:102-106`). The dispatcher collapses both arms
into `IoError` and throws the distinction away. Preserving it maps `NotFound` back to
`KeyNotFound` and leaves `TransportError` as `IoError`.

**Measure anyway, as confirmation with a prediction.** The reading is now a test of
understanding rather than a search: a solo-group run should report **non-zero** misses and a
shared-group run **zero**, on the same workload. If that does not hold, something else is
also wrong and the fix would have masked it. The harness exists — it is the same
solo-versus-shared comparison used for store declines.

## R3. Where the counters live, and what a new one costs

| Piece | Location |
| --- | --- |
| Counter storage | `apps/certus-server-yaml/src/metrics.rs` — `ServiceCounters`, `AtomicU64` each |
| Observer trait | `lib/shmq-dispatcher/src/translate.rs:41-48` — `TranslatorObserver`, all methods defaulted to no-ops |
| Increment site | `translate.rs:614` — `obs.on_lookup(hits, misses, gpu_bytes)` |
| Export | `apps/certus-server-yaml/src/telemetry.rs:111-125` — OTel observable counters |
| Wiring | `apps/certus-server-yaml/src/main.rs:290` |

The observer **is** wired in `certus-server-yaml`, so the zero reading is not unwired
plumbing. The plain `certus-server` binary passes no observer by design, which is why the
trait's methods default to no-ops — a new method therefore costs that binary nothing.

Metric names are declared with dots (`certus.lookup_misses_total`) and render to Prometheus
with underscores, which is why the name used on the cluster was correct.

## R4. Alternatives considered for the counter's source

- **Count in the server from `served_by`** — the original design's implicit assumption.
  Rejected for Phase 1: it makes the counter depend on the whole interface change, which is
  the thing being deferred. It becomes the right answer once attribution exists.
- **Count in `remote-lookup` itself.** It knows its own hit rate, and it is where a
  *responder*-side counter would have to live. Rejected as the Phase 1 source because it
  measures a different thing: what this node was asked for by peers, not what this node
  obtained from peers. Both are worth having; the requester-side one is what the open
  question in R2 needs.
- **Derive it from the existing `certus_lookup_hits_total` split by configuration** — run
  with peers and without, and difference the two. This is what was actually done on the
  cluster and it is why a counter is needed: it produced 33.9% versus 34.6%, a difference
  smaller than the run-to-run spread, and could not attribute a single block.

## R5. What must NOT regress

- **`TranslatorObserver`'s defaults.** A new method must be defaulted, or the plain
  `certus-server` stops compiling and every test host that implements the trait breaks.
- **The `ok` byte's meaning.** Phase 1 touches no wire encoding at all; this is recorded so
  that a later phase's widening is not accidentally pulled forward into it.
- **Counter monotonicity.** The counters are `AtomicU64` with `Relaxed` ordering and are
  exported as OTel *observable* counters, read at collection time. A fix that makes a
  counter decrease — for example by recomputing rather than accumulating — would break the
  export contract regardless of being arithmetically nicer.

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

### Measured — T001 predicted wrongly, T005 confirmed the fix

**T001 (before any change).** The prediction was solo > 0, shared == 0. **Both read 0.**

| arm | generator: not served | `certus_lookup_misses_total` |
| --- | --- | --- |
| solo (own RDMA group) | 605 144 | **0** |
| shared (peer reachable) | 657 927 | **0** |

**The prediction failed because the experiment chose the wrong variable, not because the
mechanism was wrong.** `--rl-group` selects which cluster a server joins; it does not decide
whether remote lookup is *wired*. The `full-remote` profile connects the receptacle either
way, so `remote_lookup.get()` succeeds and `lib.rs:2575` executes in both arms — the solo
server's log even shows `remote-lookup-rdma-initiator: connected`. Only `minimal.yaml` leaves
it unconnected, and that profile has neither SPDK nor GPU, so the generator cannot drive it.

**Therefore the hardware measurement cannot isolate this cause at all, and a unit test can.**
That is why the discriminator moved into `components/dispatcher`'s own tests, where the
receptacle and the peer's contents are both controllable. The test fails against the old code
with `Err(IoError("remote lookup: key not found"))` — the formatted string is itself the
proof that `RemoteLookupError::NotFound` was collapsed.

Recorded because it generalises: *a configuration flag that names a behaviour is not
evidence that the behaviour is reachable.*

**T005 (after the fix).** Prediction revised to: both arms report non-zero misses, because
the relabelling affected both equally. Held, and more tightly than predicted —

| arm | generator: returned / not served | counter: hits / misses | sum |
| --- | --- | --- | --- |
| solo | 353 532 / 605 635 | 353 532 / 605 635 | 959 167 |
| shared | 303 128 / 656 039 | 303 128 / 656 039 | 959 167 |

Both counters agree with the generator's independent count **exactly**, and hits + misses
equals total key references in both arms. FR-024's identity closes with zero errors.

### What this reveals about the other two holes: they did not fire

Since hits + misses already accounts for *every* reference, the two confirmed holes above
contributed **nothing** to these runs — no entry was held back (every handle resolved) and no
error other than `KeyNotFound` occurred. They are real defects in the code and remain worth
closing, but **they are latent, not active, and this fix is not evidence about them.** Saying
so matters: it would be easy to let the hardening tasks ride on this success and imply they
were verified, when the workload never exercised either path.

### One observation for later phases, not yet a claim

The shared arm has a *lower* hit rate than solo (31.6% versus 36.9%) and correspondingly more
misses, which is the same direction as the earlier solo-versus-shared comparison. It is still
not evidence about whether remote lookup serves anything, because `lookup_hits` does not
distinguish local from remote — that is exactly what the counter in R1 is for, and these two
runs also differ in timing and cache state. Noted so it is not later mistaken for a result.

## R6. ANSWERED: remote lookup serves 0.369% of what it is asked

The question this feature exists for, measured 2026-09-28 — the first reading ever
possible, because before the counter nothing could separate a local hit from a remote one.

Four instances driven in one RDMA group (node2 n0/n1, node5 n0/n1), `--until 10 --rate
inf`, cold-formatted, both hosts on the fixed build. **262 migrations**, so the
opportunity for a peer to hold a session's prefix genuinely existed.

| instance | remote hits | remote misses |
| --- | --- | --- |
| node2:9400 | 312 | 166 405 |
| node2:9401 | 898 | 169 186 |
| node5:9400 | 977 | 172 258 |
| node5:9401 | 256 | 151 242 |
| **total** | **2 443** | **659 091** |

| | |
| --- | --- |
| remote hit rate | **0.369%** of 661 534 keys forwarded to a peer |
| share of all hits | **0.81%** (2 443 of 300 076) |
| contribution to hit rate | **0.255pp** of the 31.3% overall |
| stores declined, **same run** | **27.5%** (181 258 of 659 091) |

**Why this reading can be trusted, where the earlier ones could not.** The accounting
closes against an *independent* count: per-instance remote misses sum to 659 091, exactly
the generator's own not-served figure, and lookup hits sum to 300 076, exactly its
returned-data figure. Zero transport errors, so every forwarded key got an answer and
hits + misses is the whole of what was asked.

**The trade is now measurable within one run.** Remote lookup buys 0.255 percentage
points of hit rate and costs 27.5% of stores declined — roughly 100:1 against. Previous
solo-versus-shared A/Bs put declines at ~0–1.5% without peers and 41–49% with them, and
the responder-pin hypothesis remains the leading mechanism: `remote-lookup/src/server.rs`
holds a `PinnedBatch` across an RDMA completion, and a held read-ref makes an entry
unevictable, so constant peer traffic suppresses eviction and reserves then fail.

**This supersedes "no evidence remote lookup serves anything."** That statement described
the instrument, not the system. It serves; it serves very little.

**What this does NOT establish.** One workload (`/tmp/stress-2m.yml`), whose working set
overflows the tier, with 262 migrations in 10 virtual seconds. Heavier migration or a
hotter shared prefix could do better, and "remote lookup is useless" is not supported.
What is durable is the instrument: any other workload can now be checked in one run.

### Two earlier readings that were NOT results, recorded so they are not cited as such

- **Solo/shared with an *undriven* peer: 0 remote hits.** Vacuous by construction — the
  second server was never driven, so its cache was empty and it could not serve. The
  fixture was built to trigger the relabelling bug and that same choice made the hit
  measurement meaningless. Reporting it as a negative would have been a serious over-claim.
- **All counters zero across four instances.** The generator never ran: FR-051 refused it
  because node5's agent binary was stale. A zero from a run that did not happen looks
  identical to a zero from a run that found nothing.

### Method note: node5 is configured independently, and it cost three failed runs

Its own `/tmp` helpers, its own binaries, its own build. Every fix on node2 needs
propagating, and I missed all three in turn:

1. `/tmp/stress-servers.sh` — node5 kept the old `grep "cold pool started"` as its **last
   command**, so a healthy start exited 1 and the caller's `set -e` aborted.
2. `certus-server-yaml` — needed rsync plus a `full-remote` rebuild on node5.
3. `workload-node-agent` — FR-051 refused twice. **Copy node2's binary rather than
   rebuilding**: identical bytes cannot disagree about provenance, and node5's tree holds
   an older generator source. `rm` the destination first — it is a cargo hardlink and
   `scp` fails with "dest open Failure" writing in place.

**Grep-as-last-command caused three separate false failures this session.** A script
ending in `grep` makes "nothing matched" its exit status, so a healthy operation reports
failure. Always `|| true` an informational grep.

## R3. Where the counters live, and what a new one costs

| Piece | Location |
| --- | --- |
| Counter storage | `apps/certus-server-yaml/src/metrics.rs` — `ServiceCounters`, `AtomicU64` each |
| Observer trait | `lib/shmq-dispatcher/src/translate.rs:41-48` — `TranslatorObserver`, all methods defaulted to no-ops |
| Increment site | `translate.rs:614` — `obs.on_lookup(hits, misses, gpu_bytes)` |
| Export (OTLP) | `apps/certus-server-yaml/src/telemetry.rs:111-125` — OTel observable counters |
| Export (`/metrics`) | `apps/certus-server-yaml/src/metrics.rs::serve_metrics` — **hand-rolled Prometheus text, and this is the one that gets scraped** |
| Wiring | `apps/certus-server-yaml/src/main.rs:290` |

The observer **is** wired in `certus-server-yaml`, so the zero reading is not unwired
plumbing. The plain `certus-server` binary passes no observer by design, which is why the
trait's methods default to no-ops — a new method therefore costs that binary nothing.

Metric names in the OTel path are declared with dots (`certus.lookup_misses_total`) and would
render to Prometheus with underscores.

**There are TWO export paths and conflating them costs a run.** An earlier version of this
table listed only the OTel one, and a counter added there alone was invisible to
`curl localhost:9400/metrics` — that endpoint is served by `metrics.rs::serve_metrics`, which
writes Prometheus text by hand from `ServiceCounters` plus snapshots it takes itself.
`certus_lookup_hits_total` appears on it because `serve_metrics` renders it, **not** because
OTel exports it. A new counter must be added to whichever path will actually be read, and
adding it to both is cheap.

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

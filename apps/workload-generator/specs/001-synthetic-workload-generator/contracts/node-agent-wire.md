# Contract: Generator ↔ Node Agent Wire Protocol

**Version**: 3
**Status**: Draft
**Spoken by**: `workload-gen` (client) and `workload-node-agent` (server), both
via `workload-wire`.

The generator cannot reach a remote Certus directly — the only ingress is a
host-local shared-memory mailbox — so each node runs an agent that accepts work
over TCP and submits it to its own local mailbox. **Only keys cross the
network.** The agent reconstructs block payloads from the key (spec FR-047).

**Every node speaks this protocol, including the local one** (FR-079). The
generator has no mailbox path of its own: a local run drives an agent the
generator launches as a child process on this host, so there is one transport,
one driver and one cleanup mechanism rather than two of each.

## Why TCP, and why this is not a bottleneck

Sustained batch rate equals pipelining depth divided by round-trip time. At 1M
keys/second with 64-key batches — beyond anything yet measured on this hardware
— that is 15,625 batches/second, which at a 50 µs round trip needs **0.78
batches in flight**. A depth of 8 is an order of magnitude of headroom, and the
payload is ~8 bytes per key, so 1M keys/second is 8 MB/s. Bandwidth is never
the constraint; round-trip latency is, and pipelining covers it.

RDMA was rejected: it solves a bandwidth and CPU-overhead problem this path
does not have, at the cost of coupling the tool to specific NICs. See
`research.md` D2.

## Transport

- TCP, `TCP_NODELAY` set on both ends. Nagle would batch small frames and add
  latency to exactly the messages whose latency matters.
- One connection per lane, or a multiplexed connection with correlation ids.
  Either is conforming; the depth of pipelining MUST be configurable and MUST
  be independent of lane count, because FR-072 requires transport concurrency
  not to affect the plan. **Configurable means a parameter, not necessarily a
  command-line flag** — what the requirement is about is that depth never
  reaches the producer, and what establishes that is a test varying it and
  comparing what crossed the wire. See `cli.md` on why no flag is exposed.
- All integers little-endian and explicitly sized. No padding, no native
  alignment assumptions.

## Framing

Every message is a 12-byte header followed by a body.

```text
Header {
  len:    u32,   // body length in bytes, excluding this header
  opcode: u16,
  flags:  u16,
  corr:   u32,   // correlation id, echoed in the reply
}
```

A reader MUST NOT trust `len` without bound: a frame larger than the configured
maximum is a protocol error and MUST close the connection rather than allocate.

## Opcodes

```text
Hello       (1): req  { proto_version:u32, build_id:[u8;32], mailbox_len:u16,
                        mailbox:[u8] }
                 resp { proto_version:u32, build_id:[u8;32], channels:u32,
                        block_bytes:u32, status:u16 }

SubmitTurn  (2): req  { session:u64, flags:u16, n:u32, [key:u64]*n }
                 resp { resident:u32, pending:u32, missing:u32, granted:u32,
                        blocks_read:u32, blocks_written:u32, elapsed_ns:u64 }

Stats       (5): req  { }
                 resp { counters:[u64;15], n_ops:u16,
                        [ op_kind:u8, requests:u64, hist_len:u32,
                          hist:[u8] ]*n_ops }

Drain       (3): req  { }
                 resp { pending:u32 }

Shutdown    (4): req  { }
                 resp { ops_submitted:u64, ops_failed:u64 }

ClearCache  (6): req  { }
                 resp { entries:u64, error_len:u32, error:[u8] }
```

### `ClearCache` exists because the generator has no mailbox (v3)

FR-046 lets a run clear the memory tier **once, before the timed window
opens**, and the generator used to issue `CLEAR_MEMORY_TIER` itself. Under
FR-079 it has no mailbox of its own, so the clear has to be asked for.

`error` empty means the clear happened and `entries` is what it dropped. A
**non-empty `error` is not a zero count**: "0 entries dropped" is a legitimate
answer for an already-empty cache, so a service that cannot clear MUST say why
rather than answer as though it had. A run that believed its cache was cold
when it was not would report a plausible hit rate for a different experiment,
which is the failure class the constitution's measurement principles name. The
generator therefore refuses to run on a non-empty `error`, and
`Service::clear_cache`'s default is that refusal rather than a success.

It is setup and MUST NOT be issued inside the timed window or mid-run: clearing
then would be the generator evicting on the eviction policy's behalf (FR-043).
The client refuses it while any turn is outstanding, for the same reason the
other synchronous calls are refused — and here it also means a clear cannot
race a store the run has already issued.

### `SubmitTurn` carries a key path, not an operation (v2)

Version 1 sent **one operation per frame** — `{ op_kind, session, keys }` —
with the generator deciding each operation and the agent relaying it. FR-072a
makes that impossible: a turn's operations are not known until the cache has
answered, because a client offers its whole path and stores what came back
absent. Relaying would also mean six or more network round trips per turn,
which FR-072b rejects: the reactive rule is cheap at `/dev/shm` latency and
ruinous over a fabric.

So a frame now carries **one turn's key path**, root of the prefix through the
end of the new growth, and the agent performs the check, the loads and the
stores against its own local mailbox. `flags` bit 0 requests an event poll
after the turn, which is the plan's decision and so stays with the generator.

**What this does and does not move across the boundary.** The workload stays
entirely on the generator: which keys, in which order, for which session, at
which virtual time. What the agent applies is the *client rule* — load what is
resident, store what is absent — which is a mechanical consequence of the
cache's answer and not a workload decision. Version 1's concern, that deciding
"what to issue" on the agent would put workload semantics on two sides of a
network boundary, still holds and is still respected.

**The rule MUST have exactly one implementation**, and since FR-079 that is
structural rather than a discipline. If the local path and the agent each had
their own, the two execution paths could diverge and FR-072's guarantee — that
the same description and seed produce the same workload whichever path runs it
— would become unverifiable. There is now nowhere for a second implementation
to live: the executor, the opcode mapping and the payload buffer are in
`workload-node-agent`, and the generator depends on none of them.

That also fixed a dependency arrow the earlier design had accepted as a cost.
The agent used to depend on `workload-gen`'s library for the split and the
encoders — the wrong direction for a daemon, taken deliberately for the
stronger property. It now owns them, which is both the tidier shape and what
lets `workload-gen` drop `shm-queue` and CUDA and become a workspace default
member. None of it can live in `workload-wire`, which is a CUDA-free default
member: depending on `shmq-dispatcher` from there would unify `interfaces/spdk`
into every plain `cargo build`.

`op_kind` keys the `Stats` reply's histograms and is **the mailbox's own opcode
number** — the opcode the agent sent, since the agent is what timed it. The
names for those numbers are in `workload-wire` beside the field, because one
end fills it and the other prints it, and `workload-node-agent` — the only
crate that can see both — pins them to `shmq-dispatcher`'s constants in a test.
That pin is what makes the duplication safe rather than a second source of
truth: the failure it prevents is silent, a run reporting a `LOOKUP` percentile
under the name `RESERVE`.

### The mailbox-facing code is the only collector

Latency and bandwidth MUST be measured by whatever code talks to the
shared-memory mailbox — which since FR-079 is **always the agent**, on every
node — and MUST NOT be inferred from this wire. Only the agent is near a
mailbox, so only the agent can time a `LOOKUP` or count a block that moved; a
figure derived from wire timings would include the network and describe the
transport rather than Certus.

This is also what makes routing the local node through an agent free for the
numbers that matter. The loopback hop is outside every reported figure, because
the agent times its own mailbox requests; what it costs is the rate at which
the generator can *feed* work, which the plan queue's underrun count (FR-062)
and paced mode's lateness (FR-080) are there to catch.

Because there is exactly one executor, the counters mean the same thing
wherever they were gathered, which is what makes one node's number and
another's comparable. Had each path counted for itself, a local/remote
difference would be unattributable between the cache and the instrument.

`Counters` carries the fifteen figures the report needs — requests, key
references, the three `CHECK` states, `LOOKUP` hits and misses, attempted and
declined counts for reserve, transfer and commit, and the blocks actually read
and written. Bandwidth follows from the last two: a block is read on a `LOOKUP`
hit and written on an accepted `COPY_TO_STORE`, and no control operation moves
a byte (FR-066). Counters **sum** exactly across nodes, which is why bandwidth
can be totalled where a percentile cannot.

They travel back **for reporting only**. Nothing the generator does may depend
on them: they do not gate validity, steer submission, or re-enter the workload.

### Ordering: one causal rule for the generator, everything else is Certus's

The generator owns exactly one ordering property, and it is **causal** rather
than general: turn *n+1* of a session contains the blocks turn *n* stored, so
two turns of the same session MUST NOT be in flight together. The agent
therefore processes one connection's frames in arrival order, and a lane owns a
fixed set of sessions on its own connection; overlap comes from having several
lanes. An agent that handed a connection's frames to a thread pool would
violate this, and the symptom would be quiet — turn *n+1* would check a prefix
turn *n* had not yet stored, find it absent, and store it again, so the run
would complete with inflated store counts and a depressed hit rate.

**Reordering after submission is Certus-internal and explicitly not a generator
concern.** The mailbox has parallel channels and the server runs multiple
threads, so two submitted requests may be processed in either order; preventing
that would mean one channel and no parallelism, which would remove the thing
the measurement exists to exercise. Neither the generator nor the agent makes
an end-to-end ordering claim, and neither should be changed to try to enforce
one. `CHECK`'s `PENDING` state is the evidence that Certus already expects
concurrent stores of one key and reports them rather than treating them as an
error.

### `Stats` returns histograms, because percentiles do not merge

The agent times each local request, so per-operation latency (FR-066a) can only
be measured there. It MUST be returned as **serialized histograms**, never as
percentiles.

Merging percentiles is arithmetically wrong: the median of two nodes' medians
is not the median of their requests, and the same holds for every quantile.
Averaging per-node p99s would produce a number that looks authoritative and
belongs to no distribution — the failure mode this project has repeatedly
caught elsewhere. HDR histograms merge exactly, so the wire carries the
histogram and the generator merges before taking any quantile.

Each entry also carries `requests` so the generator can mark a count too small
to quote (FR-066a).

**`touch` was missing from this list and from the plan, and that was a defect
rather than an omission.** It is the reference report (FR-041), and in the
shipped client it is a distinct call from the check: `batch_check` asks which
blocks are present, while `touch` is documented as "update eviction ordering
for the given keys". Without it the eviction policy never learns that a read
happened, so a recency policy would be scored against a workload in which
nothing is ever recently used — the worst available bug for an instrument built
to evaluate that policy, and one no emitted trace would reveal.

**These `op_kind` values are this protocol's own and are not the mailbox's.**
The generator-to-agent wire numbers them from zero in the order above; Certus's
shared-memory mailbox uses its own opcodes (`CHECK` 1, `TOUCH` 2, `RESERVE` 3,
`COPY_TO_STORE` 4, `COMMIT_STORE` 5, `ABORT_STORE` 6, `LOOKUP` 9, `TAKE_EVENTS`
10 — read from `lib/shmq-dispatcher/src/wire.rs`). The agent translates between
them, and a table with the mapping belongs in the agent rather than in either
protocol, so that neither can drift into silently meaning the other.

## The Hello handshake is mandatory and fail-closed

`Hello` MUST be the first message on every connection, and the generator MUST
refuse the run unless:

- `proto_version` matches exactly, and
- `build_id` matches its own.

`build_id` is a 32-byte digest of the agent binary. This is not defensive
programming for its own sake: a **stale remote binary has previously
invalidated measurements in this repository's history**, and the failure was
silent — the run completed and produced numbers. Spec FR-051 makes refusing
mandatory.

The response also carries `channels` and `block_bytes` so the generator can
check its lane count against the node's actual mailbox capacity, since the
mailbox is depth-1 per channel and over-subscription silently serialises.

## Payload reconstruction

The agent holds a **pre-filled, reusable device buffer** and stamps the key at
a known offset. It MUST NOT construct block bytes per operation (spec FR-038).

This is a performance requirement, not a style preference: filling a 64 KiB
block and copying it host-to-device costs microseconds per block, against a
measured end-to-end cost of 5.6–14 µs per key. Per-operation payload
construction would make the agent the bottleneck, and a run in which the agent
is the bottleneck measures the agent (constitution Principle I).

## Lifecycle

Started before a run, stopped after; startup and teardown lie outside the timed
window (spec FR-050, FR-067).

- **Idempotent startup.** A leftover agent from a crashed run MUST be detected
  and replaced, never reused. `Hello`'s `build_id` check makes reuse of a
  *stale* agent impossible; the launcher must additionally handle a *current*
  leftover.
- **Verified teardown**, including when the generator dies abnormally. Spec
  FR-053. A teardown bug that left resources held previously invalidated an
  entire A/B series in this repository, and it failed nondeterministically
  rather than visibly, so teardown MUST be checked rather than assumed.
- **Only launching goes over ssh.** Load is driven over this transport. An
  ssh-batch harness cannot sustain continual load (spec FR-054). The **local**
  node is launched without ssh at all (FR-079): ssh to `localhost` is
  unnecessary, and on a host whose own key is not trusted it is refused, which
  would make the simplest possible run need setup.

## Failure handling

If a connection breaks or `Hello` fails mid-run, the generator MUST abort the
whole run and report it invalid, naming the node (spec FR-064). It MUST NOT
continue on the surviving nodes: a lost node makes its sessions' prefixes
unreachable, shrinks the set of migration targets, and redistributes load onto
the survivors, so any number produced afterwards describes a different
experiment than the one requested.

## Conformance tests

The wire format MUST be tested without a live agent and without an accelerator:

1. Round-trip encode/decode for every opcode, including a zero-key
   `SubmitTurn`.
2. An oversized `len` is rejected without allocating.
3. `Hello` with a mismatched `build_id` is refused, and the refusal names the
   node.
4. `Hello` with a mismatched `proto_version` is refused.
5. A truncated frame mid-body is an error, not a partial parse.
6. Submitting against a loopback agent stub yields the same operation sequence
   as the local path for the same plan — the executable form of FR-072. Because
   the rule is now applied on the agent, this test is also what proves the two
   paths share one implementation rather than two that happen to agree today.
7. Two histograms merged from a `Stats` reply give the same quantiles as one
   histogram of the same samples, and a merge of per-node **percentiles** does
   not — asserted so the wrong method cannot be reintroduced as an
   optimisation.
8. A `ClearCache` that did not happen does not encode like one that dropped no
   entries, and `Service::clear_cache`'s default is the refusal — so a service
   with no mailbox cannot answer as though it had cleared one.

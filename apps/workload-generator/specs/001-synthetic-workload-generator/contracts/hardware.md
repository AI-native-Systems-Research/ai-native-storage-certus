# Contract: The Hardware File

**Version**: 1
**Status**: Draft — agreed, not yet implemented (FR-081, FR-082)
**Read by**: `workload-gen run`
**Example**: `hardware.example.yml`, which a test keeps parseable

A run needs two kinds of fact, and this is the second kind. The **workload
description** says what the sessions do and is portable across clusters
unchanged (FR-005). The **hardware file** says what to run it against, and is
portable across nothing — it is about one deployment.

Keeping them apart is the whole point. Put a mailbox path or a pacing rate in a
description and it stops being the same workload on the next cluster, while
still loading and still producing numbers.

## Location

`./cluster.yml` by default; `--hardware <file>` names another. If neither
exists, a run uses built-in defaults — this host, port 7420,
`/dev/shm/certus-shmq`, rate 1.0 — and reads no file, so the simplest
invocation needs no configuration.

**A file read without being asked for is announced** on the error stream before
the run starts, and the report records the file's path and digest either way
(FR-063). An implicit pickup that changed a run's meaning silently would be the
failure this specification keeps naming: a plausible number for a different
experiment rather than an error.

## Schema

```yaml
version: 1

# Target-dependent scalars. Each may be overridden on the command line.
rate: 1.0                 # virtual seconds per wallclock second; .inf = work-conserving

# The Certus instances to drive. An instance is NOT a machine.
instances:
  - host: node5
    port: 7420
    mailbox: /dev/shm/certus-numa0
  - host: node5
    port: 7421
    mailbox: /dev/shm/certus-numa1
```

`version` is required and is refused if it is not understood, for FR-006's
reason applied here: a file whose meaning has changed must not be read under
the old meaning.

`rate` accepts `.inf`, which is YAML's own spelling for infinity and the one
the description already uses for an unbounded lifetime. It means
work-conserving — see FR-080, and note that a large finite rate is **not** the
same thing.

Each entry of `instances` needs a `host`; `port` defaults to 7420 and `mailbox`
to `/dev/shm/certus-shmq`, so a homogeneous cluster is a list of hostnames.

## An instance is identified by (host, mailbox, port)

A machine routinely runs several Certus instances — one per NUMA domain, or one
per NVMe device as `deploy/multi-instance/` does, with mailboxes
`/dev/shm/certus-shmq-0`, `-1`, … So a hostname is not an identity (FR-081),
and two entries may legitimately share a `host`.

What they may **not** share is a `port`: each instance needs its own agent, and
two agents on one port means the second one's startup finds the first, takes it
for a leftover of a crashed run and shuts it down (FR-052). A run would then
drive one instance while reporting two. **Duplicate `(host, port)` pairs MUST
be refused**, naming both entries.

Two co-resident instances are two independent caches. A session migrating
between them is a genuine cache miss (FR-048, FR-049) and a legitimate
experiment, not a no-op.

### Unless the instances share: remote lookup changes what a migration means

That independence is a property of the **deployment, not of this generator**.
With Certus's remote-lookup feature enabled, the instance a session arrives on
may fetch the whole prefix from the instance it left, over the fabric — so the
migration is a remote *hit*, and the run measures the remote path instead of a
cold prefix. Both are legitimate experiments, but they are different ones, and
the same description and seed produce both.

**The generator cannot currently tell them apart.** Nothing on the wire
distinguishes a local hit from a remote one: the counters carry residency, not
provenance. So a migration under remote lookup does not merely behave
differently, it *reads* as an ordinary hit, and two runs with identical reports
can have measured different things — the same hazard as an implicitly picked-up
hardware file, which is why that one must be announced and digested.

A `remote_lookup:` field is deliberately **not** specified here. Nothing could
validate the declaration, and an unvalidated field that can be silently wrong
is worse than a documented gap — the same reasoning that makes an unknown field
a refusal rather than a default. The fix is served-by tier attribution, which
is specified separately; until it exists, an operator comparing runs across
server profiles has to know which profile each run used.

There is a second-order effect, recorded because it is easy to mistake for a
defect: with remote lookup on, the turn immediately after a migration can race
the last turn on the old instance, since the two sit on different connections
and nothing orders them. Whether the prefix has landed decides whether the
remote fetch finds it. The exposure is bounded by the migration count, which
the report already prints, and it belongs to a class the methodology already
answers — concurrent stores of one key are expected (`CHECK` reports
`PENDING`), races are preserved deliberately (FR-035, FR-036), and
hit-dependent comparisons therefore need n >= 8 runs. It needs no new
machinery.

## Precedence

The command line overrides the file, **per field**, and `--instance` replaces
the file's whole instance list rather than adding to it. The overriding case
that matters is a rate sweep: it varies the rate while holding the deployment
fixed, so the rate has to be settable without editing the file.

A field absent from both falls back to the built-in default.

## What is deliberately not here yet

Named so that the format's growth is visible rather than accidental:

- **per-instance CPU/NUMA affinity**, which needs an affinity option on
  `workload-node-agent` first. It matters: an agent serving a NUMA-local
  mailbox can otherwise land its threads and its device buffer in the wrong
  domain, and cross-socket DMA has been measured on this hardware at 16%
  run-to-run variance — a measurement hazard, not untidiness.
- **per-instance GPU device**, for the same deployment shape.
- **per-instance agent binary**. Unlikely ever to belong here: the handshake
  requires every instance to run the same build (FR-051), so a per-instance
  binary is a misconfiguration the protocol already refuses.

Nothing about the workload will be added, ever. That is FR-005.

## Relation to the repository's other conventions

This is a **deliberate departure**, recorded rather than discovered later.
Deployment elsewhere in this repository is env-var-shaped: an ordered host list
in `CERTUS_TEST_NODES`, scalars in `CERTUS_TEST_SHM_PATH` and friends, and a
documented caller contract in `scripts/lib/cluster-launch.sh`. YAML is chosen
for extensibility — the pacing rate is already a second kind of field, and the
affinity fields above are foreseen.

`certus-server-yaml`'s YAML profiles are **not** related and are the wrong hook
to extend: they are build-time component composition, selected by
`CERTUS_PROFILE` when the server is compiled, and carry no host, port or
mailbox at all.

One thing worth knowing: the deployment layer already generates an equivalent
table. `deploy/multi-instance/launch-servers.sh` writes an `instances.tsv` of
`IDX  BDF(s)  NUMA  SHM_PATH  POLLER_CPU`. Consuming it directly is not
required, but a hardware file whose fields cannot be filled from that table
should be treated as suspect — it means one of the two has the deployment
wrong.

## Conformance tests

1. A file with two instances on one host, on different ports, is accepted and
   both are driven.
2. Two entries sharing `(host, port)` are refused, naming both.
3. `--instance` on the command line replaces the file's list; `--rate`
   overrides the file's rate.
4. An unknown `version` is refused; an unknown field is refused rather than
   ignored, so a typo is not silently a default.
5. `rate: .inf` yields a work-conserving run, and the report names the mode.
6. A `./cluster.yml` read implicitly is announced, and its path and digest
   appear in the report.
7. Nothing belonging to the workload description is accepted here.

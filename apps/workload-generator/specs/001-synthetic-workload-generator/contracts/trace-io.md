# Contract: The Invocation Record

**Version**: 1
**Status**: Draft
**Produced by**: `workload-gen emit`
**Normative for**: `workload-trace`, and any tool reading a file it wrote.

An emit run writes **projections** — one other tool's format per requested
flag — and every one of them is written from a single per-turn record. This
contract specifies that record and the invariants it satisfies. What each
target format does with it is `trace-interop.md`.

The record is not itself written to disk. It exists so that the three
projections are written from one description of a turn rather than three, which
is what lets them be compared to each other and to the report's own counts
(FR-056). A projection is not a trace: it is lossy on purpose, not
self-describing, and never accepted in place of the description and seed when
reproducibility is being checked (FR-072, FR-075b).

**Scope: emitting only.** Nothing here specifies a reader for a captured trace.
Reading real traces belongs to fitting, which is out of scope (spec, Out of
scope).

## The invocation record

**One record per invocation** — one turn of one session, not one session. A
session of 135 turns is 135 records, tied together by `session_id` and
`invocation_index`.

| Field | Type | Meaning |
| --- | --- | --- |
| `trace_id` | string | the run's identifier |
| `session_id` | string | the session this turn belongs to |
| `invocation_index` | int64 | 0-based position within the session |
| `parent_invocation` | int64 | previous turn, or −1 at a session root |
| `request_start` | double | virtual seconds, run-global clock |
| `request_end` | double | always **null** — see below |
| `timestamp_kind` | string | always `start` |
| `timestamp_is_synthetic` | bool | always `true` — the clock is virtual |
| `model` | string | null; this feature does not model models |
| `input_length`, `output_length` | int64 | **tokens** — `blocks * block_size` |
| `reuse_from` | list\<int64\> | invocation indices whose blocks this re-reads |
| `new_input_blocks` | list\<u64\> | input keys first minted at this turn |
| `new_output_blocks` | list\<u64\> | output keys first minted at this turn |
| `full_input_blocks` | list\<u64\> | the complete ordered input key list |
| `full_output_blocks` | list\<u64\> | the complete ordered output key list |
| `partial_final_valid` | int64 | always **null**: this generator mints whole blocks |

Lengths are in **tokens** rather than blocks because that is the unit every
target format states them in, so converting at the boundary would mean each
projection re-deriving the same figure and being able to disagree about it.

**`request_end` is null, not equal to `request_start`.** A turn occupies a
single virtual instant because nothing here models service time, so a duration
of zero would be a measurement this generator did not make. The same rule as
the emit report's omitted fields (FR-071): a zero is indistinguishable from a
real result.

A fan-in form of `parent_invocation` is **absent**, because a session's chain
is a path and not a graph (spec, Out of scope). A consumer must treat the
absence as empty rather than as unknown.

## One record per turn means every projection streams

An emit run walks virtual time and writes each record's projections as its turn
is simulated. Nothing is buffered, and every output is in clock order.

This is worth stating because the alternative is a real trap: a format that
groups by session cannot write a session's row until that session **ends**, so
emitting it means holding every in-flight session in memory. At the concurrency
this feature targets that is hundreds of megabytes, held to produce a file that
is being streamed to disk anyway. See `trace-interop.md` for the format where
this was measured.

## Keys are chained u64, not dense mint-order integers

A key is the 64-bit chained value `contracts/key-derivation.md` specifies,
because stateless prefix derivation is what removes a global trie from the
per-key hot path. It is **not** an index in creation order.

Published formats generally do number blocks densely, so a projection onto one
of them either carries our identifiers as opaque 64-bit values or declares the
divergence as a loss (FR-057, FR-077). Anything that treats an identifier as an
index into a dense array will break on these outputs, which is why no output
implies density it does not have.

## Invariants every record satisfies

FR-058 requires these to hold on every record, not on a sample:

1. `len(full_input_blocks) * block_size == input_length`, and likewise for
   output — exact, with no rounding, because every block is whole.
2. `full_input_blocks` of turn *n* begins with the whole of turn *n−1*'s input
   followed by its output — the chain is append-only.
3. `new_input_blocks` is the suffix of `full_input_blocks` not present in any
   earlier turn of the session; the prefix before it is exactly the reuse.
4. Every key in `full_*` is reproducible from `key-derivation.md` given the
   description and seed — which is what lets a consumer verify keys it did not
   produce.
5. `partial_final_valid` is in `1..=block_size`, or null when the final block
   is whole.
6. `request_start` is non-decreasing across the run.

## Versioning

The field names above are pinned. If they have to change, that is a **new
version** and never an edit — the same rule and the same reason as
`key-derivation.md`'s Versioning section: an edit produces an output that
loads, replays, and means something different.

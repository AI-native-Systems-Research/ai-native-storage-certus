# Contract: Emitted Trace

**Version**: 1
**Status**: Draft
**Produced by**: `workload-gen emit`
**Normative for**: `workload-trace`, and any tool that reads a trace this
feature wrote.

This is the shape an emit run writes. One schema, two containers holding
identical records (spec FR-055), plus a manifest that tells a reader what the
trace supports without the reader having to recognise which trace it is
(FR-056).

**The schema is not ours.** It is the normalisation layer another team built
over the public LLM-trace formats, and the corpus at `traces/` (24 traces) is
already in it. We adopted it rather than inventing one, so a generated workload
and a real trace are interchangeable inputs to the same analysis — which is the
whole point of emitting a file at all. `research.md` D8 records why it is the
right superset, and `trace-interop.md` records what we convert it *to*.

**Scope: emitting only.** The full schema also describes a delta encoding and a
`field_status` capability system, which exist because real traces disagree
about what they carry. We always write the same thing — the full encoding, with
every field we support populated — so this contract specifies that case and
does not re-specify the reader side. Reading real traces in this schema belongs
to fitting, which is out of scope (spec, Out of scope).

## Layout

```text
<output_dir>/
  manifest.json                              # written LAST (see Completeness)
  invocations/block_size_<N>/part-*.jsonl     # or .parquet, or both
```

`block_size` counts **blocks of keys** as this feature mints them, and appears
in the path so the directory is self-locating even if the manifest is lost.
There is no `blocks/` directory: block role is a property real traces recover
from text, and we do not model it.

## The invocation record

**One record per invocation** — one turn of one session, not one session. A
session of 135 turns is 135 records, tied together by `session_id` and
`invocation_index`.

| Field | Type | Meaning |
| --- | --- | --- |
| `trace_id` | string | the run's identifier, from the manifest |
| `session_id` | string | the session this turn belongs to |
| `invocation_index` | int64 | 0-based position within the session |
| `parent_invocation` | int64 | previous turn, or −1 at a session root |
| `request_start`, `request_end` | double | virtual seconds, run-global clock |
| `timestamp_kind` | string | always `start` |
| `timestamp_is_synthetic` | bool | always `true` — the clock is virtual |
| `model` | string | null; this feature does not model models |
| `input_length`, `output_length` | int64 | **blocks**, not tokens and not bytes |
| `reuse_from` | list\<int64\> | invocation indices whose blocks this re-reads |
| `new_input_blocks` | list\<u64\> | input keys first minted at this turn |
| `new_output_blocks` | list\<u64\> | output keys first minted at this turn |
| `full_input_blocks` | list\<u64\> | the complete ordered input key list |
| `full_output_blocks` | list\<u64\> | the complete ordered output key list |
| `partial_final_valid` | int64 | valid units in a trailing partial block |

`parent_invocations` (the fan-in form) is **absent**, because a session's chain
is a path and not a graph (spec, Out of scope). A reader must treat an absent
`parent_invocations` as empty rather than as unknown.

## One record per line means the file streams

An emit run walks virtual time and writes each record as its turn is simulated.
Nothing is buffered, and the file is in clock order.

This is worth stating because the alternative is a real trap: a format that
groups by session cannot write a session's record until that session **ends**,
so emitting it means holding every in-flight session in memory. At the
concurrency this feature targets that is hundreds of megabytes, held to produce
a file that is being streamed to disk anyway. See `trace-interop.md` for the
format where this was measured.

## Keys are chained u64, not dense mint-order integers

The corpus's own traces number blocks densely in creation order. We do not: a
key is the 64-bit chained value `contracts/key-derivation.md` specifies,
because stateless prefix derivation is what removes a global trie from the
per-key hot path.

The manifest therefore declares `block_id_space` (FR-057), and a consumer must
read it rather than assume density. Anything that treats an identifier as an
index into a dense array will break on our traces, and it should break loudly
at the manifest rather than silently at the first key.

## The manifest

Written **last**. Every field a reader needs to interpret the records:

| Field | Value we write |
| --- | --- |
| `trace_id` | the run's identifier |
| `source_class` | `pre_hashed` — we mint keys, never text |
| `provenance` | `synthetic` |
| `id_semantics` | `rolling_prefix` |
| `block_id_space` | `chained_u64` — **not** dense mint order (FR-057) |
| `key_derivation_version` | the version in `key-derivation.md` |
| `block_size`, `block_sizes_available` | block geometry |
| `time_unit`, `time_origin` | `seconds`, `run_start` |
| `timestamp_kind`, `timestamp_is_synthetic` | `start`, `true` |
| `encoding` | `full` — both `full_*` and `new_*` populated |
| `censoring` | `{left: false, right: true}` — the span ended the run |
| `tokenizer_name`, `chat_template_id` | null; there is no text |
| `description_digest`, `seed` | reproduction parameters (FR-072) |
| `block_stats` | sessions, invocations, distinct keys |

## Invariants every row satisfies

FR-058 requires these to hold on every emitted row, not on a sample:

1. `len(full_input_blocks) == input_length`, and likewise for output.
2. `full_input_blocks` of turn *n* begins with the whole of turn *n−1*'s input
   followed by its output — the chain is append-only.
3. `new_input_blocks` is the suffix of `full_input_blocks` not present in any
   earlier turn of the session; the prefix before it is exactly the reuse.
4. Every key in `full_*` is reproducible from `key-derivation.md` given the
   manifest's salt fields — which is what lets a consumer verify a trace it did
   not produce.
5. `partial_final_valid` is in `1..=block_size`, or null when the final block
   is whole.
6. `request_start` is non-decreasing across the file.

## Completeness is structural, not a flag

The manifest is written last, so **a directory without one is incomplete by
construction** (FR-073). There is no "complete" flag to get wrong, and a run
that dies of a full filesystem leaves a directory that no reader will accept.

## Versioning

The record and manifest field names above are pinned. If they have to change
once traces exist, that is a **new version** recorded in the manifest, never an
edit — the same rule and the same reason as `key-derivation.md`'s Versioning
section: an edit produces a trace that loads, replays, and means something
different.

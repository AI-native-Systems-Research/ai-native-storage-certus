# Contract: Trace Input and Output

**Version**: 1
**Status**: Draft
**Consumed by**: `certus-workload fit`, `certus-workload validate`
**Produced by**: `certus-workload plan` output modes 2 and 3

One schema, two containers. This is both the format `fit` reads from real traces and the format
the generator emits when it is not talking to a server, so that a generated workload and a real
one are directly comparable and interchangeable as inputs to any third-party tool.

**Trace collections in this format are not part of this repository.** They are large, come from
many sources under differing licences, and which ones are on hand will change; a path is supplied on
the command line. So what is normative here is the **format**, and every trace is self-describing
through its own `manifest.json` — a reader learns what a trace supports by reading it, never by
recognising the trace. Observations below are drawn from traces examined while writing this contract
and are given by character rather than by name, so that nothing depends on a particular file.

## Layout

```
<trace_id>/
  manifest.json                                  # required; self-describing
  invocations/block_size_<N>/part-*.parquet       # one row per LLM request
  blocks/block_size_<N>/part-*.parquet            # block_id -> role; MAY be empty
  sample_block_size_<N>.jsonl                     # optional; a few rows, for eyeballing
```

`block_size` counts **tokens**, not bytes, and appears in the path so one trace can carry several
blockings of the same source. `block_size_0` means the trace has no block data at all.

## The invocation record

| Field | Type | Meaning |
| --- | --- | --- |
| `trace_id` | string | which trace this row belongs to |
| `session_id` | string | conversation / agent run. **Nullable**; absent in some real traces |
| `invocation_index` | int64 | 0-based position within the session |
| `parent_invocation` | int64 | predecessor, or −1 |
| `parent_invocations` | list\<int64\> | predecessors when there is more than one; see § Fan-in |
| `request_start`, `request_end` | double | seconds; origin per manifest. Either MAY be null |
| `timestamp_kind` | string | `start` \| `submission` |
| `timestamp_is_synthetic` | bool | whether times were invented |
| `model` | string | nullable |
| `input_length`, `output_length` | int64 | **tokens**, not blocks and not bytes |
| `reuse_from` | list\<int64\> | invocation indices whose blocks this one re-reads |
| `new_input_blocks` | list\<int64\> | input blocks first minted here |
| `new_output_blocks` | list\<int64\> | output blocks first minted here |
| `full_input_blocks` | list\<int64\> | the complete ordered input block list |
| `full_output_blocks` | list\<int64\> | ditto for output |
| `partial_final_valid` | int64 | valid tokens in the trailing partial block; nullable |

Block IDs are **dense integers in mint order**, not hashes, and are **global to the trace**. They
map onto `CacheKey` directly.

## Two encodings, and they are mutually exclusive

A reader MUST detect which is in use — `full_input_blocks` non-empty versus empty — and MUST NOT
assume either. Both rules below were verified exhaustively — every row, not a sample — against real
traces of each kind.

**Delta encoding** (`source_class: raw_text`). Only newly-minted blocks are listed;
`full_input_blocks` is empty. Reconstruct:

```
full_input(n) = concat over a in reuse_from(n) of (new_input(a) ++ new_output(a)) ++ new_input(n)
```

with the invariant `len(full_input(n)) == input_length(n) / block_size`, rounded down.
*Verified on 10 916/10 916 rows of an agentic tool-use trace.*

**Full encoding** (`source_class: pre_hashed`). `full_input_blocks` is complete; the delta fields
are empty. Invariants:

```
(input_length - partial_final_valid) % block_size == 0
len(full_input_blocks) == (input_length - partial_final_valid) / block_size + 1
```

*Verified on 12 031/12 031 rows of a pre-hashed production trace.*

**The trailing partial block is handled differently by the two**, and this is the trap: the delta
encoding **excludes** it, the full encoding **includes** it with `partial_final_valid` giving its
valid token count. A reader that assumes one convention is silently off by one block per request on
the other — which is how the full-encoding rule above was found, after the delta invariant failed
on 12009 of 12031 rows.

## Sharing is carried by global block IDs, not by `reuse_from`

`reuse_from` is **intra-session compression only**. Genuine sharing *between* sessions appears as
two sessions listing the same global block ID — 16188 of 364645 minted blocks in
one agentic trace examined. A reader that treats `reuse_from` as the sharing signal will conclude
there is no cross-session sharing at all, which is wrong. In a production conversational trace, a
single block appeared in **every one** of its 12 031 invocations — a universal shared prefix.

## Fan-in

`parent_invocations` MAY carry more than one predecessor. It is rare — in the traces examined only a
single map-reduce-shaped benchmark had it, at fan-in 2 — and in every such row **`reuse_from` was
empty**, so it is a *scheduling* dependency rather than prefix reuse. Readers MAY ignore fan-in for
cache purposes, and the generator does not emit it (spec Out of Scope).

## `manifest.json`

Load-bearing fields. `source_class` selects the encoding; `id_semantics` MUST be `rolling_prefix`
for the trace to be usable by `fit`; every trace examined that carried blocks declared it, which is
what makes this format a good match for the generator's own key model (spec FR-008).

| Field | Use |
| --- | --- |
| `source_class` | `raw_text` \| `pre_hashed` \| `metadata_only` — selects encoding, or says there are no blocks |
| `id_semantics` | `rolling_prefix` required for structural fitting |
| `block_size`, `block_sizes_available` | tokens per block |
| `provenance` | `production` \| `benchmark` \| `benchmark_execution` — a benchmark trace characterises the benchmark |
| `field_status` | per-field `native` \| `reconstructed` \| `unavailable`. **Consult before fitting**: a `reconstructed` value is weaker evidence than a native one |
| `supports` | capability summary; `B` = block roles, `R` = reuse structure, `T` = timing, `V` = token counts. **`P` is undocumented and its meaning is not established — readers MUST NOT depend on it** |
| `role_codes` | the role vocabulary for the `blocks` table |
| `block_stats` | per-block-size session / invocation / unique-block counts |

## What `fit` can take from which trace

| `source_class` | What it means | Fittable |
| --- | --- | --- |
| `raw_text` | tokenised from source text; blocks reconstructed | everything, plus block roles |
| `pre_hashed` | block IDs supplied by the source | structure and arrival; roles unavailable |
| `metadata_only` | no block data at all (`block_size_0`) | **arrival and token-length distributions only** — nothing structural |

Two properties disqualify a trace from parts of a fit regardless of its class, and both are readable
from `field_status` rather than from the trace's identity. A trace with a null `session_id` cannot
supply `turns`, `growth_per_turn`, or the sticky root binding of FR-009a, because requests cannot be
grouped into sessions. A trace with no timestamps gives no arrival model, and its reuse statistics
depend on file order, so `fit` MUST report them as order-dependent rather than as measured.

## Output modes

`plan` emits the same schema, so a generated workload is substitutable for a real one:

1. **Direct to a Certus server** — the runner path; no file. The only Certus-specific mode.
2. **`.jsonl`** — one invocation record per line. Human-readable; the natural choice for small
   plans and for diffing.
3. **Parquet** — the same records, columnar, partitioned as `invocations/block_size_<N>/`.

Modes 2 and 3 MUST write a `manifest.json` alongside, with `source_class: pre_hashed`
(the generator knows every block ID it minted, so the full encoding is the honest one),
`id_semantics: rolling_prefix`, `provenance: synthetic`, and `timestamp_is_synthetic: true`.
They MUST use the **full encoding** and MUST populate `partial_final_valid`, so that a reader
applying the rules above gets the documented invariants. Neither mode involves Certus in any way.

Note that `certus-workload plan`'s native artifact remains `events.bin`
(`contracts/plan-format.md`): it is fixed-width, indexable, and streamable, which these formats are
not. Modes 2 and 3 are interchange, not a replacement.

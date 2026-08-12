# Contract: Trace Input and Output

**Version**: 1
**Status**: Draft
**Consumed by**: `certus-trace fit`, `certus-trace validate`
**Produced by**: `certus-workload emit` (JSONL) and `certus-trace convert` (parquet) — see spec FR-021h

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

## One schema, two containers, two population patterns

Three things are easy to conflate and are worth separating, because only one of them needs a
decision from a reader:

- **Container** — parquet or JSONL. Same records either way; JSONL is one object per line. A reader
  supports both (§ JSONL is a container, not a lesser format).
- **Population pattern** — delta or full, below. This is **not a second schema**: every column
  exists in every invocations file, and what differs is which ones are *populated*. So a reader
  needs one parser with a branch, not two parsers.
- **Capability** — what the trace can support, declared per field in the manifest's `field_status`
  and unrelated to either of the above.

A reader SHOULD **normalise on ingest**: reconstruct full ordered block lists once, at the boundary,
and let everything downstream see a single representation. Otherwise the delta/full branch leaks
into every statistic that walks a block list, and each such site becomes a place to get the
trailing-partial-block convention wrong.

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

## JSONL is a container, not a lesser format

A JSONL file holds **the same records** as the parquet — one JSON object per line, same field names.
It is not an abbreviated or lossy view, and a reader MUST accept either container for any operation.
Two details, both measured on real traces:

- **`block_size` may appear per record**, which parquet leaves to the path and the manifest. It is
  redundant. A reader MUST reject a file whose per-record `block_size` disagrees with the manifest or
  the path rather than picking a winner, since the three disagreeing means the file's block lists
  cannot be interpreted at all.
- **`parent_invocations` may be absent** where it would be empty in every record. A reader MUST treat
  an absent `parent_invocations` as empty, not as unknown. Traces that actually have fan-in do carry
  it.

**But a JSONL file may be a sample, and that is the hazard.** The `sample_block_size_<N>.jsonl` files
that ship alongside a parquet trace exist for eyeballing and are *tiny* — 6 lines against 1 960 074
parquet rows in one measured trace, 136 against 2 115 623 in another, with every sampled row present
in the parquet. Fitting from one would produce a confident-looking model derived from six requests.

So a reader MUST distinguish *a full trace that happens to be in JSONL* from *a sample of a trace*,
and the manifest makes that checkable: `block_stats.<block_size>.invocations` declares how many
records the trace has. A tool that consumes fewer than that MUST say so, and `fit` MUST refuse
outright (spec FR-055e). Naming alone is not a sufficient test — a `sample_` prefix is a convention,
not a guarantee — so the count is the test.

## Output modes

The generator emits this same schema, so a generated workload is substitutable for a real one.
Which binary provides which mode follows the dependency rather than convenience (spec FR-021h):

| Mode | Provided by | Notes |
| --- | --- | --- |
| 1. **Direct to a Certus server** | `certus-workload-run run` | No file. The only Certus-specific mode |
| 2. **`.jsonl`** | `certus-workload emit` | One record per line. Needs nothing beyond `serde_json`, so it stays in the generator |
| 3. **Parquet** | `certus-trace convert` | Same records, columnar, partitioned as `invocations/block_size_<N>/`. A columnar writer would otherwise put `arrow` in a crate that `cargo test --all` builds every run |

Mode 3 living elsewhere is not a compromise: FR-021c already requires modes 2 and 3 to be producible
from an existing `events.bin` **without regenerating**, so conversion was always independent of
generation, and `convert` merely names where that independence lives.

Modes 2 and 3 MUST write a `manifest.json` alongside, with `source_class: pre_hashed`
(the generator knows every block ID it minted, so the full encoding is the honest one),
`id_semantics: rolling_prefix`, `provenance: synthetic`, and `timestamp_is_synthetic: true`.
They MUST use the **full encoding** and MUST populate `partial_final_valid`, so that a reader
applying the rules above gets the documented invariants. Neither mode involves Certus in any way.

**Modes 2 and 3 MUST NOT emit warmup requests.** A warmup window says which operations a *report*
excludes (FR-045); it is a property of a measured run, not of a workload, and this schema gives an
invocation no field in which to say it was one. Emitting them unmarked would make the trace a
different stream from the plan's own report — measured against a warmed plan, request length and the
unique-keys curve diverged by exactly the extra requests, while an unwarmed plan round-tripped at
exactly zero. So the emitted trace is the plan's **measured window**, and the emitter states how many
requests it withheld.

Nothing is lost from the native artifact: `events.bin` keeps the `WARMUP` flag on every one of those
events, and a consumer that wants the warmed stream reads the plan rather than the interchange copy.

One consequence to expect rather than debug: the reuse-distance CDF of a warmed plan and of its
emitted trace are **close but not identical**. A warmup reference really did occupy the consumer's
capacity, so the plan counts it inside the reuse distance of whatever follows it, and the trace does
not contain it at all — so the trace's distances are shorter, one-directionally, by the warmup
references that sat between two measured references to one key. The other three FR-056 statistics are
unaffected and round-trip exactly.

Note that `certus-workload plan`'s native artifact remains `events.bin`
(`contracts/plan-format.md`): it is fixed-width, indexable, and streamable, which these formats are
not. Modes 2 and 3 are interchange, not a replacement.

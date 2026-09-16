# Contract: Trace Interoperability

**Version**: 1
**Status**: Draft
**Applies to**: `workload-gen convert`
**Companion to**: `trace-io.md`, which specifies the native emitted trace

Every format here is a **conversion**, never a second thing an emit run writes.
That rule comes from `research.md` D1 and it is what keeps consumer-specific
shapes out of the published trace: an emit run writes one schema in two
containers (spec FR-055), and `convert` projects that into whatever a
particular tool eats.

**Every claim below was verified against upstream bytes**, not recalled. Where
a detail is unverified it says so, and that is a task, not a footnote.

## The compatibility ladder

The corpus's `source_class` field turns out to be the whole map, because each
rung has a different de-facto format and a different loss:

| `source_class` | Carries | Upstream examples | What is lost coming from us |
| --- | --- | --- | --- |
| `metadata_only` | arrivals + counts | Azure, BurstGPT | all reuse structure |
| `pre_hashed` | **block identity** | Mooncake, WekaTrace | varies; see below |
| `raw_text` | prompt text | WildChat, ragbench | nothing — but needs a tokenizer |

We are natively `pre_hashed`, so that rung is where interop is cheap and
lossless enough to be worth doing.

**There is no standards-body format at this level.** OpenTelemetry GenAI
semantic conventions are the only governed standard anywhere nearby and they
sit at the `raw_text` rung. Everything below is de-facto adoption.

## Two properties any target must be judged on

Both were learned by measuring a format that fails one of them:

1. **A run-global clock.** Cross-session interleaving *is* the workload as far
   as a cache is concerned — it is what makes two sessions contend for the same
   shared object at the same time, and what sets the concurrent footprint. A
   format whose timestamps are per-session cannot carry it, and every replayer
   then invents arrivals differently.
2. **A run-global key scope.** Shared-object pools exist to produce reuse
   *between* sessions. A format whose identifiers are scoped per session cannot
   express that at all, no matter how faithfully it records each session.

## Mooncake — `convert --to mooncake` (the recommended target)

`github.com/kvcache-ai/Mooncake` @ `main:FAST25-release/traces/`:
`conversation_trace.jsonl`, `synthetic_trace.jsonl`, `toolagent_trace.jsonl`.

JSONL, **one document per line, one line per request**, exactly four fields:

```json
{"timestamp": 0, "input_length": 6758, "output_length": 500,
 "hash_ids": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]}
```

Verified on 2 328 rows of `conversation_trace.jsonl`:

- `timestamp` is **milliseconds**, non-decreasing. Their corpus quantises it to
  a 3 000 ms tick (delta histogram 3000×151, 2999×54, 3001×53 — a 3 s tick with
  ±1 ms jitter), 6–12 requests per tick, 774 s span, 3.0 req/s. **The
  quantisation is their corpus's, not the format's**, so we write true
  millisecond values from the virtual clock.
- `len(hash_ids) == ceil(input_length / 512)` on **2 328 of 2 328** rows, so
  block size 512 and — note — **ceil**: the trailing partial block gets an
  identifier.
- `hash_ids` are **globally dense across the whole file**: range 0..44 683 with
  44 684 distinct values, i.e. perfect dense mint order at **global** scope.
- **Identifier 0 is the first identifier of all 2 328 rows** — a universal
  shared system prefix, which is cross-session sharing expressed in the format.
- No `session_id`, `model`, think time, TTFT or service time.

It satisfies both properties in § Two properties: per-request rows on one
global clock, and globally scoped identifiers. That is why it is the
recommended target.

**What the conversion loses, and what it must therefore declare:**

- **Session grouping.** There is no `session_id`, so sessions survive only as
  the prefix structure of `hash_ids`. Recoverable by a consumer only up to
  ambiguity, and not recoverable at all where two sessions share a prefix.
- **Input/output separation.** One list covers the whole prompt, so a turn's
  output blocks reappear folded into the next turn's prefix, as they do
  upstream.
- **`partial_final_valid`.** Their convention is ceil, which gives the partial
  block an identifier and records nothing about how full it is. **Unrecoverable
  on import**, so a round trip through this format is lossy and must not be
  used as a determinism check.
- **Dense identifiers.** Their `hash_ids` are dense mint-order integers and
  ours are chained u64 (`trace-io.md`, § Keys are chained u64). The converter
  must **renumber**, keeping one dense identifier per distinct key for the
  whole output so that global scope survives. Renumbering per session would
  destroy the sharing and is the one mistake that would look like success.

## libCacheSim CSV — `convert --to cachesim`

`github.com/1a1a11a/libCacheSim` @ `develop:doc/quickstart_cachesim.md`.
`trace_type` values are `vscsi`, `csv`, `txt`, `oracleGeneral`, plus a generic
`binary`.

At the cache level our plan is `(time, key, size)` triples, which is what this
world runs on, and it unlocks published eviction baselines to compare against.

**Columns are configurable, so there is no fixed layout to match.** Parameters
go to `-t` / `--trace-type-params`, comma-separated and quoted, keys
`time-col`, `obj-id-col`, `obj-size-col`, `obj-id-is-num`, `delimiter`,
`has-header` (`format` is binary-only). Columns are **1-based**. Verified
example:

```bash
./bin/cachesim ../data/cloudPhysicsIO.csv csv lru 1gb \
  -t "time-col=2, obj-id-col=5, obj-size-col=4, obj-id-is-num=true"
```

So the converter picks a layout and the contract documents the `-t` string to
use. Two verified traps:

- **Numeric identifiers need `obj-id-is-num=1`** or the reader errors out with
  `detect obj_id is numeric, please specify -t 'obj-id-is-num=1'`.
- **CSV is ASCII-only**; UTF-8 is not supported. Harmless for us — every column
  we write is a number — but it must not be "fixed" by adding a text column.

Lossy by design: session identity and turn structure are gone. For eviction
studies that is the right projection, not a compromise.

### `oracleGeneral`, and the baseline it unlocks

`oracleGeneral` is the format upstream recommends — binary, "a few times faster
than csv trace and uses less DRAM" — storing **time, obj-id, size,
next-access-time (in reference count)**.

Next-access-time is an *oracle* field. A real trace can only get it by a full
offline pass, but **an emit run knows the entire future of its own trace**, so
it is one backward pass over the plan. That makes optimal-policy (Belady)
baselines available for our eviction comparisons, which is a capability the
native format does not give anyone.

**Unverified**: the doc describes those four fields in prose only. The exact
struct layout, field widths and endianness have **not** been read from the
reader source. That check is a task and must happen before any bytes are
written.

## WekaTrace — never an output; reading it is DEFERRED

`huggingface.co/datasets/semianalysisai/cc-traces-weka-062126`: one 1.85 GB
`traces.jsonl`, **one line per session**, 393 lines. Card figures: 393 traces,
56 798 main turns, 98 827 total requests, 21.6 G input and 106.5 M output
tokens.

```text
{id, models[], block_size: 64, hash_id_scope: "local", requests: [...]}
```

each request `{t, type, model, in, out, hash_ids[], api_time, ttft,
think_time}`, where `type` is `s` (main stream), `n` (nested) or `subagent` — a
group carrying `agent_id`, `subagent_type`, `duration_ms`, `status`, nested
`requests[]`.

**We must never write this format, and reading it is deferred** — a real corpus
is valuable but this feature already has 24 traces in the emitted schema, and
importing is a different axis from emitting. Two measured reasons never to
write it:

1. **`first t = 0.0` for every session** (checked on all 6 sessions parseable
   from the first 20 MB), and the schema has **no field for session start**. So
   the corpus carries no cross-session arrival information and fails property
   1. Emitting it would discard our interleaving; preserving it would mean
   extending someone else's schema with a field they would not read.
2. **`hash_id_scope: "local"`** — dense per-trace identifiers, which fails
   property 2. Cross-session sharing cannot be expressed.

Also, being grouped by session, a line cannot be written until the session ends
(§ One record per line, `trace-io.md`), and a line is large: the first is
**2.76 MB** for 135 requests, because `hash_ids` repeats the whole prefix on
every request.

**As a source it is valuable**, which is why the analysis is kept rather than
discarded: 393 real agentic sessions with subagent structure. If the reader is
ever built, these are the measured notes it needs:

- `len(hash_ids) * 64 == in` exactly on every main request — full encoding, no
  delta.
- Input and output blocks are not distinguished, so an importer must not invent
  the split.
- **102 of 130 main requests in one session are strict prefix extensions of
  their predecessor, and 27 diverge.** The append-only chain plus context
  trimming — independent corroboration of the turn model (spec FR-035), from a
  corpus this feature was not fitted against.
- `hash_id_scope` must be **checked, not assumed**: a future revision declaring
  a global scope would change what an import means.

## `metadata_only` — considered and NOT adopted

The Azure `AzurePublicDataset` and BurstGPT shape: `request_start`,
`input_length`, `output_length`, and no block identity at all. Trivial to
project, and it is what a lot of serving-throughput tooling consumes.

It would be trivial to write, and it is still **not worth having**: everything
about reuse is gone, and reuse is the one thing this feature exists to model.
The single argument for it — placing our arrivals beside published real ones —
does not need a converter, because arrival rate is a parameter of the
description rather than something recovered from output. Recorded so the idea
is not re-proposed as free.

## OpenTelemetry GenAI — deliberately not implemented

The only real standard here, and still the wrong tool. Recorded because the
reasoning is not obvious and someone will propose it again.

The repo's own corpus generator is
`benchmarks/kv-offload-otel-replay/trace-gen/trace_to_otel.py`; the loader is
`benchmarks/kv-offload-replay/otel_corpus.py`. One JSON **file** per
conversation, `{trace_id, span_count, collected_at, spans: [...]}`, each span
carrying `gen_ai.request.model`, `gen_ai.usage.input_tokens`,
`gen_ai.usage.output_tokens`, `gen_ai.request.max_tokens`,
`gen_ai.input.messages` and `gen_ai.output.messages` — the message lists as
JSON strings.

- **The format is not the problem.** `start_time`/`end_time` are absolute ISO
  8601 and express arbitrary interleaving natively, so property 1 is
  satisfiable. The existing *corpus* fails it anyway: `BASE = datetime(2026, 1,
  1, 0, 0, 0, UTC)` is a module constant and every conversation starts at `t =
  BASE`, so its timestamps look absolute while being session-relative.
  `load_otel_convs` then keeps only per-conversation gaps and discards absolute
  time entirely.
- **It carries no block identity**, so the cache structure would have to live
  in fabricated *text*. And the direction is the opposite of the intuitive one:
  you cannot make text that produces chosen identifiers, because
  `rolling_prefix` means an identifier is a hash of (parent hash, its tokens)
  computed by the engine. What you control is the sharing structure — literal
  shared token prefixes make the engine's hashes coincide.
- **That requires exact block alignment**: each block's filler must tokenise to
  precisely `block_size` tokens, which pins one tokenizer and one chat template
  into the artifact. Upstream's own `gen_text` samples random vocabulary
  identifiers and decodes them, and its docstring says "~n_tokens (within a few
  %)" — detokenise-then-tokenise is not an involution. Harmless upstream, where
  reuse is intra-conversation only; fatal for cross-session sharing, which
  needs the counts exact.
- **Cost**: roughly 50× size inflation — 21.6 G tokens fit in 1.85 GB as
  identifiers and would be about 85 GB as text.
- **No accelerator is required to get keys back out.** Tokenise, block, and
  rolling-hash is pure CPU work. A GPU is needed only to drive a real engine,
  which is the one thing this format would be *for*.
- Nonsense filler is adequate for storage measurement — cost follows token
  counts and prefix identity, and `max_tokens` imposes the lengths — and
  useless for anything content-dependent: answer quality, natural stopping,
  guardrail routing, speculative-decode acceptance.

**Conclusion**: implement only if driving a real inference engine becomes a
goal, and then as a lossy secondary output with the tokenizer pinned in the
manifest.

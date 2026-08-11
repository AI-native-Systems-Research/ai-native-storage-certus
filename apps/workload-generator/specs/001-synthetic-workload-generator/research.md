# Research: Synthetic KV Workload Generator

**Status**: Partial — the trace measurements below are complete; the derivations listed under
§ Open derivations are not yet done.

This file holds the evidence and derivations behind `spec.md`. Nothing here is normative: where a
requirement follows from a measurement, the requirement states the structural conclusion and this
file records how it was reached.

## A note on the traces measured

Everything in § Trace measurements was measured on a collection of real LLM traces that is
**deliberately not part of this repository** — it is large (~800 MB), draws on many sources under
differing licences, and is only a subset of what may be used. It was a **sample of convenience**. It
is named here because provenance is what makes evidence assessable, and "an agentic trace" is
unfalsifiable; but no requirement in `spec.md` depends on any of these files existing, and `spec.md`
deliberately cites none of them. Someone re-running these measurements on a different collection
should expect the *shapes* to recur and the *numbers* to differ.

The format itself **is** normative and is specified in `contracts/trace-io.md`.

## Trace measurements

### Inventory

24 traces, each a directory with a self-describing `manifest.json`. By `source_class`:

| `source_class` | Count | Traces |
| --- | --- | --- |
| `raw_text` | 11 | `exgentic_{appworld, browsecompplus, swebench, tau2_airline, tau2_retail, tau2_telecom}`, `paper_review_dag`, `ragbench`, `ragbench_canonical`, `swe_agent`, `wildchat` |
| `pre_hashed` | 6 | `mooncake_{agent, conv}`, `qwen_{code, reasoning, tob, toc}` |
| `metadata_only` | 7 | `azure_{llm_code, llm_conv, mm_multi, mm_single, mm_text}`, `burstgpt_{api, conv}` |

All 17 traces carrying block data declare `id_semantics: rolling_prefix`, which is the same key
model as FR-008. The 7 `metadata_only` traces have `block_size_0` and empty block lists throughout:
they can support an arrival and token-length fit and nothing structural.

### The two block encodings

Verified exhaustively — every row, not a sample.

**Delta encoding** (`raw_text`). `full_input_blocks` empty; only newly-minted blocks listed.

```
full_input(n) = concat over a in reuse_from(n) of (new_input(a) ++ new_output(a)) ++ new_input(n)
len(full_input(n)) == input_length(n) // block_size
```

Verified **10 916 / 10 916** rows of `exgentic_tau2_airline` at `block_size` 16, zero mismatches.

**Full encoding** (`pre_hashed`). `full_input_blocks` complete; delta fields empty.

```
(input_length - partial_final_valid) % block_size == 0
len(full_input_blocks) == (input_length - partial_final_valid) / block_size + 1
```

Verified **12 031 / 12 031** rows of `mooncake_conv` at `block_size` 512, zero mismatches.

**How the second rule was found, which is the useful part.** Applying the delta invariant to the
full-encoded trace failed on **12 009 of 12 031** rows — off by exactly one block. The two encodings
differ on the trailing partial block: the delta form excludes it, the full form includes it with
`partial_final_valid` giving its valid token count. A reader assuming either convention is silently
off by one block per request on the other, which is why `contracts/trace-io.md` requires detection
rather than assumption.

### JSONL against parquet: same records, sample-sized coverage

Six `sample_block_size_<N>.jsonl` files ship beside parquet traces. Compared field-by-field and
row-by-row against the corresponding parquet:

| Trace | JSONL lines | Parquet rows | Coverage | JSONL-only field | Parquet-only field |
| --- | --- | --- | --- | --- | --- |
| `wildchat` | 6 | 1 960 074 | 0.0003% | `block_size` | `parent_invocations` |
| `swe_agent` | 136 | 2 115 623 | 0.006% | `block_size` | `parent_invocations` |
| `paper_review_dag` | 3 | 300 | 1% | `block_size` | — |

Every sampled row was located in the parquet by `(session_id, invocation_index, input_length)`: 6/6,
136/136, 3/3. So the JSONL is a strict sample of the same data, not a different view of it.

The two field differences are both benign and both need a stated rule:

- `block_size` per record is **redundant** — parquet leaves it to the path and the manifest. Three
  sources that can disagree, so a reader rejects on disagreement rather than picking a winner.
- `parent_invocations` is absent exactly where it would be empty in every record. `paper_review_dag`,
  the one trace with real fan-in, **does** carry it. So absence means empty, not unknown.

**The conclusion that matters**: JSONL loses nothing *per record*, so it is a legitimate container for
a full trace — and the generator emits it (FR-021a mode 2), which makes reading it necessary for the
FR-058a round trip. But these particular files are eyeball samples, and fitting a model from six
requests would succeed while meaning nothing. Hence FR-055e: partiality is judged by comparing
records consumed against `block_stats.<block_size>.invocations`, because a `sample_` filename prefix
is a convention rather than a guarantee.

### Shape taxonomy

Normalised to **tokens**, since block counts are not comparable across traces with different
`block_size`. Medians unless stated.

| Character | Trace | Invocations | Sessions | Turns | Shared prefix | Private tail |
| --- | --- | --- | --- | --- | --- | --- |
| Agentic, tool-heavy | `exgentic_appworld` | 48 453 | 1 500 | 32.3 | 93 088 tok | 208 tok |
| Agentic, long-context | `exgentic_swebench` | 91 768 | 1 959 | 46.8 | 18 272 tok | 288 tok |
| Agentic, transactional | `exgentic_tau2_airline` | 10 916 | 957 | 11.4 | 7 616 tok | 192 tok |
| Agentic, SWE | `swe_agent` | 2 115 623 (probed 250 000¹) | 9 661¹ | 25.9 | 9 136 tok | 240 tok |
| Production code assistant | `qwen_code` | 43 011 | 26 406 | 1.6 | 1 072 tok | 768 tok |
| Production chat | `qwen_toc` | 43 058 | 23 101 | 1.9 | 720 tok | 352 tok |
| Production chat | `wildchat` | 1 960 074 (probed 249 216¹) | 86 975¹ | 2.9 | 128 tok | 160 tok |
| Retrieval / RAG | `ragbench` | 67 351 | 67 351 | 1.0 | 384 tok | 16 tok |
| Pre-hashed conversational | `mooncake_conv` | 12 031 | —² | —² | 512 tok | 2 560 tok |

¹ the probe read a prefix of these two traces, so their session counts and width profiles are lower
bounds; the invocation totals are the traces' true sizes, read from the parquet metadata.
² `session_id` is null, so sessions cannot be recovered and `turns` is unfittable.

**Two regimes, ~60× apart in shared-prefix length.** Agentic traces are sharing-dominated (7.6k–93k
tokens shared against a 200–290 token private tail); chat is private-dominated (128–1072 shared
against 160–768 private). A model fitted only on one misses the other entirely, which is why
`spec.md` treats coverage of both as a test-matrix concern rather than picking a single fit target.

**Block-size invariance check.** `exgentic_tau2_airline` measures 59 blocks shared at `block_size`
128 (= 7 552 tokens) and 476 blocks at `block_size` 16 (= 7 616 tokens) — agreement to 0.8%. This is
evidence the delta reconstruction is correct, since an error in it would not cancel across two
blockings of the same source.

### Cross-session sharing versus intra-session reuse — the ceiling on remote lookup

**The § Shape taxonomy "shared prefix" column conflates two things**, and separating them changes what
it means. That column is the longest common prefix against *all* earlier requests, which includes the
requesting session's **own** earlier turns. For a cache that is one number; for **remote** lookup it
is the wrong one, because a session's own history is local under any placement that keeps a session on
one node. Only *cross-session* sharing can be served remotely.

Re-measured, tracking which sessions have touched each block, so a block counts as cross-session only
if some *other* session touched it first:

| Trace character | p50 LCP vs any earlier | p50 LCP vs **other sessions** | cross / any |
| --- | --- | --- | --- |
| Agentic, transactional | 476 blk | **292 blk** (4 672 tok) | 61.3% |
| Agentic, long-context | 1 360 blk | **468 blk** (7 488 tok) | 34.4% |
| Production code assistant | 67 blk | **8 blk** (128 tok) | 11.9% |
| Production chat | 9 blk | **1 blk** (16 tok) | 11.1% |
| Retrieval / RAG | 24 blk | **24 blk** (384 tok) | 100% |

(`block_size` 16 throughout; the long-context trace probed at 60 000 invocations.)

Two conclusions:

1. **Remote lookup's ceiling varies by roughly 300× across workload classes** — 468 blocks per request
   of remotely-servable prefix in the long-context agentic case against 1 block in chat. A remote-cache
   result measured on a chat-shaped workload says almost nothing about the agentic case, and vice
   versa. This is the single most important number for characterising the remote-lookup feature, and
   it is why the multi-node test matrix has to span classes rather than pick one.
2. **RAG is the clean case at 100%**: every session is a single request, so *all* of its sharing is
   cross-session by construction. It is the natural first workload for a remote-lookup measurement
   because nothing about the result depends on placement keeping sessions together.

The generator can already express the *aggregate* of this through `topology.self_affinity`. What no
trace can supply is the **placement** that produces it — see § What the traces cannot say about nodes.

### Is cross-session sharing bursty or diffuse? — the shape behind FR-018e

The cross-session ceiling says *how much* remote lookup could serve. This says *in what pattern*, which
determines whether it faces thundering herds or steady demand — and it is measurable without any node
information, because it is a question about time rather than placement.

For each block first touched by session A and later by a **different** session B: the gap from A to B,
and how many distinct sessions touch it within 10 s of first use.

| Trace character | Cross-shared blocks | p50 gap | p90 gap | p50 herd ≤10 s | p99 herd |
| --- | --- | --- | --- | --- | --- |
| Agentic, transactional | 738 406 | 550 s | 878 428 s | 3 | 8 |
| Agentic, long-context | 4 455 166 | 7 270 s | 105 233 s | 3 | 6 |
| Production code assistant | 988 009 | 2 576 s | 6 138 s | 2 | 6 |
| Production chat | 708 | 9 s | 2 181 s | 2 | 2 |

**Sharing is diffuse, not bursty.** Minutes to hours pass between the first and second session touching
a shared block, and herds are 2–3. That is **template-shaped** — a system prompt or tool definitions
that many sessions independently start from, spread over time — not **fan-out-shaped**, where one
parent's children would all hit a fresh deep prefix within seconds.

So the agent-fan-out workload that motivates remote lookup **does not appear in this data**, which is
why FR-018e disables it by default and requires the Test Matrix to label it as a modelled hypothesis.

**The confound, which makes this weak evidence for absence rather than proof.** The agentic traces are
benchmark *executions*. If the harness runs agents sequentially rather than concurrently — plausible,
and not something the manifests record — it would inflate every gap and flatten every herd even if the
production system fans out hard. A negative result from a serialised harness is uninformative about a
concurrent deployment. Two further caveats: the p90 gaps of 10-29 hours indicate these traces span long
wall-clock periods with idle stretches, so gap percentiles are sensitive to how the collection was
assembled; and the chat trace's 708 cross-shared blocks are too few for its 9 s median to mean much.

What would settle it is a trace with node or GPU attribution, or one collected from a concurrent agent
deployment with real arrival times. Neither is in hand.

### What the traces cannot say about nodes

**No trace in the collection carries node or GPU attribution of any kind.** There is no field for
which node served a request, and none of the manifests describes one. So the entire multi-node axis —
which node asks for which key, how sessions map to nodes, whether an agent's children land elsewhere —
is **unfittable from this data and must be declared rather than inferred.**

What the traces *do* bound is what any placement could achieve, via the cross-session table above:
remote lookup can only ever serve content some other session touched first, so that column is a hard
ceiling on remotely-servable traffic regardless of how nodes are assigned. That is a genuinely useful
constraint from data that has nothing to say about topology directly.

### Trunk width is piecewise — the measurement behind FR-009e1

Fanout events, defined as `width(d+1)/width(d) > 1.8` while more than 20% of requests are still
alive at that depth:

| Trace | Fanout events | Longest run of *constant* width |
| --- | --- | --- |
| `exgentic_appworld` | depth 1 (65→139, 2.1×) **and depth 23 (214→458, 2.1×)** | depths 109–148, width 639 (**40 depths**) |
| `exgentic_tau2_airline` | depth 124 (136→291, 2.1×) | depths 1036–1056, width 78 (21 depths) |
| `exgentic_tau2_retail` | depth 110 (311→580, 1.9×) | depths 140–155, width 521 (16 depths) |

`exgentic_appworld` also has 102 flat runs of ≥4 consecutive depths; `tau2_airline` 89;
`tau2_retail` 46.

**Why this refuted the scalar `branch_factor`.** A constant width across 40 consecutive depths means
every node in that band has exactly one child. A uniform fanout of even 1.05 would widen width by
1.05⁴⁰ ≈ 7× across those levels. So the scalar's shape is not a coarse approximation of the measured
one — it is a different shape. Hence the piecewise `branching` profile.

**Two fanout events with a flat region between is the load-bearing case**: a global preamble shared
by everything, then per-branch commonality (a tool definition, a retrieved document) shared only
within a branch, then the private tail. Two sessions on one branch share far more than two on
different branches, and no single `shared_depth` expresses that.

**Why a fitted scalar looked plausible anyway.** It averages long flat runs against rare large
jumps. Measured means: 1.009 (`tau2_airline`), 1.015 (`appworld`), 1.042 (`swebench`), 1.078
(`qwen_code`), 1.439 (`swe_agent`) — but 7.63 (`mooncake_agent`), 50.0 (`ragbench`), 81.9
(`wildchat`), where the value is not a trunk width at all but the residue of one enormous near-root
jump. The estimator is crude: it averages ratios over depths and so mixes trunk growth with sessions
ending, which is why the profiles below were measured directly rather than trusted to it.

### Width and occupancy by depth

`distinct nodes / requests reaching that depth`, so the ratio is sessions per distinct node.

`qwen_code` (`block_size` 16, 43 011 invocations):

| depth | 0 | 1 | 2 | 4 | 8 | 16 | 32 | 64 | 128 | 256 | 512 | 1024 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| nodes | 155 | 4 902 | 6 643 | 5 781 | 11 470 | 12 139 | 12 025 | 12 369 | 10 275 | 7 438 | 4 413 | 65 |
| occupancy | 277.5 | 8.8 | 6.5 | 7.2 | 3.5 | 3.3 | 3.1 | 2.7 | 2.8 | 3.0 | 3.2 | 1.2 |

`ragbench` (`block_size` 16, 67 351 invocations):

| depth | 0 | 1 | 2 | 4 | 8 | 16 | 32 | 64 | 128 | 256 | 512 | 1024 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| nodes | 1 | 1 | 9 761 | 21 242 | 21 660 | 21 459 | 11 127 | 2 721 | 1 340 | 979 | 311 | 103 |
| occupancy | 67 351 | 67 351 | 6.9 | 3.2 | 3.1 | 3.0 | 3.0 | 3.1 | 3.1 | 3.1 | 4.1 | 5.0 |

Both show: a very narrow shared preamble (1–155 nodes) for the first one or two levels, one
explosive fanout (31× and 9 761× respectively), then a long plateau.

**Bearing on `target_occupancy = 4` (FR-009g1).** Occupancy below the fanout settles at **3.0–3.2**
and holds across hundreds of depths. That is just below the chosen target, which is the right side
for a floor to design against. It is *consistent with* the judgement and does not establish it: two
traces from a convenience sample are not a population, and the spec was corrected to say so.

**Bearing on the root boundary (FR-055c).** Taking `roots.count` literally as the depth-0 width
gives 1 for `ragbench` and 155 for `qwen_code`, and then the 9 761× and 31× fanouts immediately
below have to be expressed as trunk branching, where they fail the occupancy floor at any useful
depth. Placing the boundary below the near-root fanout gives ~4 900 roots for `qwen_code` with a flat
profile beneath — true and realisable. This absorption works only for *near-root* fanout: the depth-124
event in `tau2_airline` is beyond any choice of boundary, which is the independent argument for the
profile.

### The branching segmentation rule — the derivation behind FR-055a, FR-055b and FR-055c

The measurements above used `width(d+1)/width(d) > 1.8` while >20% of requests were still alive.
Both numbers were chosen to make the structure visible. This section replaces them. Nothing in the
measurements depends on it: the flat-run evidence holds under any threshold, and this rule finds
*more* structure than 1.8× did, not less.

Measured with `research/width_profile.py` (profile) and `research/segment.py` (rule), 2026-08-11.

**1. The model's own domain makes the estimator one-sided.** Schema rule 8 requires `fanout >= 1` at
every depth, so that every trunk node has a child and the trunk is unbounded in depth. An observed
*decrease* in width therefore cannot be a fanout — it is censoring by session retirement, and it
carries no information about branching. An observed *increase* cannot be produced by censoring,
since retirement only ever removes visited nodes. So

```
f(d) = max(1, w(d) / w(d-1))
```

is one-sided, and the fitted profile is a **lower bound** on the true fanout: where censoring partly
cancels a real fanout the estimate is too small, and it can never be too large. The direction matters
and must be reported, because a trunk fitted narrower than reality generates *more* sharing than the
trace had.

This is also why no jump threshold is needed to *identify* an event. Width is an integer count, so
any increase is at least one extra node. The question a threshold was standing in for is not "is this
a fanout" but "do two adjacent depths share one fanout", which is step 2.

**2. The resolution comes from the generator, not from taste.** The generator realises a non-integer
mean fanout by randomised rounding: each node gets `floor(f)` or `ceil(f)` children, taking the
higher with probability `frac = f - floor(f)`. The realised mean over the `w` nodes of a depth is
therefore a Bernoulli average, with standard error

```
se(f, w) = sqrt( frac(1 - frac) / w )        # in children per node
```

Checked against the generator: a uniform profile of `fanout: 1.05` over `roots.count: 200` and 40 000
single-turn requests predicts `se = sqrt(0.05 × 0.95 / 200) = 0.0154`, and the realised per-depth
fanout deviates from the configured one by **1.006× at p50, 1.014× at p90, 1.018× at worst** — the
p90 matching the prediction to two decimal places.

So the generator cannot reproduce a fanout distinction finer than a few percent at these widths, and
a fitted distinction finer than that describes noise. Two adjacent depths are merged unless their
fanouts differ by more than `Z = 3` such standard errors. `1.8×` is more than twenty times coarser
than this resolution, which is why it finds only the largest events — and why it reports **no fanout
event at all** for `qwen_reasoning`, whose largest ratio is exactly 1.8.

**3. The fittable range is where the data stops contradicting the model.** A segment's fanout is a
*product* over its depths, so censoring compounds through it. The share of ratios that decrease —
which the model forbids — is the direct measure of how censored a depth range is. Against cumulative
retention `survivors(d)/survivors(0)`:

| retention floor | 0.999 | 0.99 | 0.95 | 0.90 | 0.75 |
| --- | --- | --- | --- | --- | --- |
| traces with **no** forbidden decrease | 15/16 | 12/16 | 6/16 | 4/16 | 0/16 |
| worst trace's share of decreases | 4.8% | 33% | 50% | 67% | 75% |

**0.99 is the knee**: the loosest floor at which most traces produce no observation the model cannot
represent. Segmentation runs over the retained prefix; beyond it nothing is fitted and the observed
width is reported as a lower bound. `roots.count` is exempt, being a single width reading rather than
a product — it is taken at the fold boundary whatever the retention there, which is reported beside
it.

**4. The near-root fold follows from the occupancy floor (FR-055c).** Leading segments are absorbed
into `roots.count` for exactly as long as absorbing them is what keeps occupancy at the fitted
sharing depth above `target_occupancy`. So the fold goes as deep as FR-009f requires and no deeper,
and a trace with a genuinely wide but shallow trunk keeps it. A *deep* fanout event cannot be folded
at all and the rule stops rather than pretending — which is the `tau2_airline` depth-124 case.

**This reproduces the hand judgement it replaces.** The eyeballed reading of § Width and occupancy by
depth was "~4 900 roots for `qwen_code`, not 155 splitting 31 ways" and "~21 000 for `ragbench`". The
derived fold gives **4 902** and **21 760**, from the occupancy floor alone.

| Trace | depths | fitted | `roots.count` | retention at boundary | trunk segments | largest fanout |
| --- | --- | --- | --- | --- | --- | --- |
| `exgentic_appworld` | 47 230 | 169 | 635 | 0.997 | 1 | 1.000 |
| `exgentic_browsecompplus` | 16 775 | 27 | 1 145 | 0.998 | 1 | 1.002 |
| `exgentic_swebench` | 9 280 | 60 | 1 282 | 1.000 | 1 | 1.000 |
| `exgentic_tau2_airline` | 3 583 | 48 | 27 | 1.000 | 4 | 1.492 |
| `exgentic_tau2_retail` | 2 419 | 71 | 26 | 1.000 | 5 | 1.115 |
| `exgentic_tau2_telecom` | 2 265 | 74 | 24 | 1.000 | 2 | 1.031 |
| `mooncake_agent` | 247 | 1 | 4 | 1.000 | 1 | 1 638.5 |
| `mooncake_conv` | 247 | 1 | 1 | 1.000 | 1 | 7 373.0 |
| `qwen_code` | 1 612 | 2 | 4 902 | 0.999 | 1 | 1.355 |
| `qwen_reasoning` | 3 227 | 2 | 852 | 1.000 | 2 | 1.751 |
| `qwen_tob` | 4 153 | 3 | 2 283 | 1.000 | 3 | 7.051 |
| `qwen_toc` | 5 581 | 4 | 8 034 | 1.000 | 1 | 1.443 |
| `ragbench` | 5 152 | 12 | 21 760 | 0.995 | 1 | 1.004 |
| `ragbench_canonical` | 5 152 | 12 | 21 214 | 0.999 | 1 | 1.005 |
| `swe_agent` | 8 433 | 118 | 3 | 1.000 | 8 | 173.0 |
| `wildchat` | 743 | 0 | — | 1.000 | 0 | — |

Four readings, three of them consequences a `fit` implementation has to handle rather than results:

- **Profiles are compact.** 0–8 trunk segments, against the 30–74 that segmenting the whole depth
  range produces. Nearly all of that difference is censored noise, which step 3 removes.
- **Agentic traces have a trunk to fit and chat and retrieval do not.** Fitted range 27–169 depths
  for `exgentic_*` and `swe_agent`, against 0–12 for `qwen_*`, `ragbench` and `wildchat` — the same
  ~60× split the shape taxonomy found, arrived at independently.
- **`wildchat` supports no trunk fit at all** (fitted range 0): more than 1% of its requests are a
  single block, so retention breaks at depth 1. `fit` must emit the "essentially no sharing" shape —
  `roots.count` at the peak width, a flat profile — and say that is what it did, rather than report
  the depth-0 width of 2 as `roots.count`.
- **`mooncake_*` cannot have the fold decided at all.** Its `session_id` is `unavailable`, so
  occupancy has no denominator, and the enormous near-root fanout stays in the profile where it
  fails the floor. A trace without session identity can be fitted for arrival and size but not for
  trunk shape, which is `supports: R = partial` doing exactly what it says.

**Two independent criteria agree, which is the best evidence here that the rule is right.** The
retention floor comes from the model's `fanout >= 1` domain; FR-055b's high-occupancy qualification
comes from what a trace can reveal about unvisited nodes. They are unrelated arguments, and yet every
segment the retention floor admits across all sixteen traces sits at **occupancy 4.0 or above** — the
lowest being `qwen_code` at exactly 4.0 and `swe_agent`'s deepest fitted segment at 4.1. So the range
the compounding argument admits is the range the visibility argument trusts, and `segment.py` prints
the occupancy beside every fanout so the two can never be read apart.

**What is still not derived.** `Z = 3` is conventional rather than derived; the sensitivity is mild,
since the gap between a flat run and a real event is tens of standard errors, but it is a choice.
And the retention knee at 0.99 is read off sixteen traces from a convenience sample — the *form* of
the criterion follows from the model, the *value* does not.

### Fit tolerances and divergence measures — the derivation behind FR-057a and FR-057b

FR-056 names four statistics to compare between a trace and the synthetic output fitted from it, and
FR-057a requires a tolerance per statistic rather than one scalar. Neither the measures nor the
thresholds were stated. Measured with `cargo run --release -p workload-model --example seed_floor`,
2026-08-11.

**The floor a tolerance must clear.** Two plans from the *same* document differing only in `seed` are
the same workload sampled twice, so the divergence between them is irreducible. A tolerance below it
fails a model that is correct. All 28 pairs of eight seeds, three shapes from the taxonomy above,
three plan sizes:

| statistic | measure | 2 000 req | 10 000 req | 50 000 req | default |
| --- | --- | --- | --- | --- | --- |
| reuse-distance CDF (objects) | area between CDFs | 0.0243 | 0.0201 | **0.0076** | 0.02 |
| reuse-distance CDF (bytes) | area between CDFs | 0.0220 | 0.0189 | **0.0067** | 0.02 |
| prefix-sharing depth | KS | 0.1155 | 0.0508 | **0.0283** | 0.05 |
| request length | KS | 0.0630 | 0.0415 | **0.0116** | 0.02 |
| unique keys over time | max log ratio | 0.3358 | 0.1758 | **0.0846** | 0.15 |

Worst of the three shapes at each size. Defaults are 1.7–3.0× the floor at 50 000 requests, which is
therefore part of the default: every floor falls with the sample, so a bare number is loose at one
size and unreachable at another. `validate` must refuse to apply a default to a materially smaller
plan rather than compare against a floor the plan cannot reach.

**KS is the wrong measure for the primary statistic, and this is the finding.** The reuse-distance CDF
is steep where the live session population puts a large mass, and a supremum over a steep region moves
a long way for a small horizontal shift. Measured on the same seed pairs:

| shape | references | KS (sup) | area | ratio |
| --- | --- | --- | --- | --- |
| agentic | 1.1 M | 0.3074 | 0.0135 | 22.7× |
| agentic | 5.9 M | 0.1285 | 0.0031 | 41.1× |
| agentic | 30.0 M | 0.1164 | 0.0034 | 34.2× |
| chat | 6.6 M | 0.0597 | 0.0023 | 26.3× |
| mixed | 29.7 M | 0.0716 | 0.0030 | 23.7× |

A KS tolerance on the primary statistic would have to be ≥0.15 to avoid failing correct models, and
at 0.15 it would pass almost anything. The area floor is two orders of magnitude smaller and far
flatter across shapes — 0.0023 to 0.0034 at 30 M references against a KS spread of 0.06 to 0.12. So
the comparison gates on the area and reports the sup beside it, a large sup next to a small area being
informative in its own right: it says the two CDFs agree in bulk and disagree over a narrow band of
distances.

The other two distributions keep KS. Their floors are already small and fall as `1/sqrt(n)` — sharing
depth 0.116 → 0.051 → 0.028 and request length 0.063 → 0.042 → 0.012 across a 25× sample increase —
so nothing is gained by changing a standard, distribution-free measure.

**Unique-keys-over-time needs a relative measure, and needs the ramp excluded.** It is a monotone
curve of counts, not a distribution, so its measure is the largest log ratio between the two curves
over the ordinals they share. Two exclusions, each removing a difference that is not a difference in
workload shape:

- points where either count is under 100, where the `1/sqrt(n)` noise on the count itself is already
  10%;
- the first 10% of the run, which is the **session-population ramp**. Its composition is
  seed-dependent by construction: at request ordinal 7 one run had accumulated 413 distinct keys and
  another 245, a log ratio of 0.52, while by ordinal 13 000 the same pair agreed to 3.9% and by 50 000
  to 0.9%. This is the same exclusion FR-045 makes for warmup, for the same reason.

Without both exclusions the floor measured **0.90 to 2.12 and identical across three plan sizes** —
and a size-independent floor cannot be sampling noise, which is what identified the measure rather
than the data as the problem.

**One hypothesis tested and refuted.** The reuse-distance floor is large and *non-monotone* in the
sample below ~6 M references (0.277 at 185 k, 0.350 at 1.1 M, 0.147 at 5.9 M, 0.065 at 30 M). The
natural explanation was the population ramp sitting inside the measured window, since these fixtures
set no `warmup`. Adding a warmup long enough to cover the ramp changed nothing — 0.42 against 0.56 at
10 000 requests, 0.099 against 0.104 at 50 000 — so the instability is the sup's sensitivity to a
steep CDF and not the ramp. Recorded because it was the obvious explanation and it was wrong.

**What is still not derived.** The safety factor over the floor (1.7–3.0×) is a judgement, and the
floors come from three synthetic shapes rather than from the traces — a real trace's floor cannot be
measured this way at all, since a trace cannot be re-sampled with a different seed.

### Cross-session sharing rides on global block IDs

- `exgentic_tau2_airline`: **16 188 of 364 645** minted blocks appear under more than one
  `session_id`.
- `mooncake_conv`: one block appears in **all 12 031** invocations — a universal shared prefix; and
  44 144 of 182 790 distinct blocks are referenced more than once.

`reuse_from` is intra-session compression only. A reader treating it as the sharing signal concludes
there is no cross-session sharing at all, which is wrong.

### Block roles

`exgentic_tau2_airline` blocks table, 364 645 rows: `tool_result` 170 764, `assistant` 87 165,
`user` 44 695, `tool_call` 33 432, `tool_definition` 27 080, `system` 1 509.

An agentic workload's reuse is dominated by **tool output**. The generator has no role concept and
this is deferred, on the grounds that the statistics driving cache behaviour (shared/private depth,
reuse distance) are role-agnostic. The distribution above is the reason to keep it recoverable.

### Fan-in

`parent_invocations` carries more than one predecessor in exactly one trace: `paper_review_dag`, 100
invocations at fan-in 2 against 200 at fan-in 0 (100 sessions × 3 invocations — two independent
roots merging into a third). **`reuse_from` is empty for every one of those merges**, so it is a
*scheduling* dependency, not prefix reuse. The generator's strict-chain assumption is therefore not
violated at the block level; what it cannot express is a request that must wait on two predecessors.

### Tokens, not bytes

`block_size` counts tokens (16/32/64/128/512 observed). Converting to KV bytes needs layer count, KV
head count, head dimension and dtype width — the manifest has a `model_config` slot and it is
**null in all 24 traces**. This is why FR-011a makes entry size a chosen input rather than a fitted
quantity: it does not affect the reference pattern, only when a consumer's storage fills.

### Reproducing this

The traces are external; with a copy on hand:

```sh
python3 -m venv /tmp/pqenv && /tmp/pqenv/bin/pip install pyarrow   # ~1 min; no system pyarrow
```

Read `<trace>/manifest.json` first to pick the encoding, then reconstruct per
`contracts/trace-io.md`. For the sharing statistics, sort invocations by `request_start` and walk
them, counting how many leading blocks of each request are already in a seen-set — with
`rolling_prefix` identity that count *is* the longest common prefix against the union of all earlier
requests, which is the FR-056 statistic.

The width and segmentation figures have that walk written down, in two steps so the rule can be
re-derived without re-reading 800 MB:

```sh
/tmp/pqenv/bin/python research/width_profile.py <trace> --out profile.json   # measurement
/tmp/pqenv/bin/python research/segment.py profile.json                      # the rule
```

`width_profile.py` reports `survivors` and `sessions` beside `width` at every depth, because the
censoring correction and the occupancy floor both need them and a width profile alone cannot be read
honestly.

## Threats to validity

1. **Convenience sample.** 24 traces, chosen by availability. Shapes are likely to recur; the
   numbers should not be treated as population estimates.
2. **Benchmark provenance.** The agentic traces are benchmark executions, so they characterise the
   benchmark's agent loop rather than production traffic. The production traces
   (`qwen_*`, `wildchat`, `mooncake_*`, `azure_*`, `burstgpt_*`) are the ones to weight for realism,
   and they are also the ones missing roles or session IDs.
3. **Order dependence.** `ragbench` and `swe_agent` have no usable timestamps, so "already seen" is
   file order and their sharing figures are indicative only. `qwen_*`, `exgentic_*`, `wildchat` and
   `mooncake_*` have real timestamps.
4. **Truncation.** `wildchat` (1 960 074 invocations) and `swe_agent` (2 115 623) were probed at
   120k–250k, i.e. 6-13% of each, so their session counts and width profiles are lower bounds. The
   sharing medians are less affected, being medians over the prefix read rather than extrapolations.
5. **The fanout threshold was arbitrary, and is no longer used.** 1.8× was chosen to make the
   structure visible. § The branching segmentation rule replaces it with a resolution derived from
   the generator's own randomised rounding; the flat-run evidence never depended on the threshold,
   and the derived rule finds more events than 1.8× did. What remains chosen there is `Z = 3` and
   the 0.99 retention knee, both named as such.

## Open derivations

Assigned to this file by `spec.md` and **not yet done**:

- **The trunk-occupancy bound and the `auto` closed form** (FR-009f/FR-009g) — full derivation, and
  the `target_occupancy = 4` choice, which § Width and occupancy by depth supports but does not
  establish.
- ~~**The segmentation rule for fitting a `branching` profile**~~ — **discharged 2026-08-11** by
  § The branching segmentation rule. No jump ratio is used: increases are events by integrality,
  decreases are censoring by rule 8, adjacent depths merge at the resolution of the generator's own
  randomised rounding, and the near-root fold falls out of the FR-009f occupancy floor. `Z = 3` and
  the 0.99 retention knee remain choices, and are named there as such.
- ~~**The four default per-statistic `fit`/`validate` tolerances**~~ (FR-057b) — **discharged
  2026-08-11** by § Fit tolerances and divergence measures. Three measures for four statistics, each
  default set 1.7–3.0× above a measured seed-to-seed floor at a stated plan size. The safety factor
  and the use of synthetic shapes rather than traces remain limitations, named there.
- **The `branch_skew` parameterisation**, and the fitting procedures for `shared_depth` and
  `roots.popularity`.
- **Reuse-distance estimation method** and the **significance-testing approach** behind `repeat: 8`.
*(Discharged by removal: the `GetIoStats` cross-check tolerance was the seventh item here. FR-042a,
FR-042b and SC-007a are out of scope — reconciling per-class byte totals against a drive-aggregated
counter requires bounding the consumer's background staging and promotion traffic, which cannot be
done without modelling how that consumer works. See spec § Out of Scope.)

Parked rather than open: an **LP/flow relaxation** for a true offline upper bound under
heterogeneous entry sizes. It was only ever needed to make Belady/OPT a sound ceiling, and OPT is
deferred with the rest of cache simulation (FR-034b).

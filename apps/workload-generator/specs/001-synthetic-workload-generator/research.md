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

### The child-choice law — the derivation behind FR-055j

`branch_skew` was the last structural parameter with no fitting procedure, and the reason it resisted
one is that the obvious target — the popularity curve over child rank — is not a property the corpus
holds in common. This section derives what to fit instead, from what the mechanism can observe.

**What the trunk mechanism reads from the child law is one scalar.** The walk carries an expected
cohort down the trunk as `cohort *= p(child taken)`, and the child taken is itself drawn from `p`, so
the expected factor at a split is `Σ_i p_i · p_i = Σ p²`. Nothing else about the law enters: the
cohort's decay, and therefore where sharing ends, is a product of collision probabilities. Its
reciprocal `n_eff = 1/Σ p²` is the inverse participation ratio that trunk occupancy, validation rule
16 and `branching: auto` were already written in terms of, so fitting this scalar keeps those four
consumers consistent by construction rather than by agreement.

Measured from a split's child fan-ins the estimator is `Σ c²/(Σ c)²`, over **all** children — a
singleton child is how a session becomes private, so excluding singletons would fit a law the
generator does not apply. The denominator is the summed child fan-in rather than the segment's own
fan-in, so a session that retired *at* the split is excluded from a choice it never exercised;
measured leakage at a split has a median of exactly 0.000 in every band of six traces, so the two
denominators barely differ in practice, but only one of them is the conditional the law describes.

**Under FR-055g's discrete Zipf the collision probability has a closed form**, `Σ_k (k^-s/H_n(s))² =
H_n(2s)/H_n(s)²`. It is strictly increasing in `s` at fixed `n` and decreasing in `n` at fixed `s`, so
inverting it at a measured target is a bisection with no local minima and no starting guess. Cost is
one pass over ranks `1..=max` accumulating both harmonic sums, so a band's fit costs its **widest**
split rather than the sum over its splits — which matters because the widest fanout in the corpus is
`wildchat`'s 204030-way one and a per-split sum would be quadratic in it.

**Why not the rank curve.** Fitting the shape directly was tried and does not transfer. A Zipf fails
the corpus's two widest fanouts in *opposite* directions — `qwen_code`'s root head is 25% too light
under any exponent that fits its tail, and its log-log slope is non-monotone, so it is not a power law
and Mandelbrot's offset cannot rescue it, while `ragbench`'s depth-1 head is 2.4x too heavy and *is*
fitted by a Mandelbrot `q = 4.7`. Below depth 1 `ragbench` is **exactly uniform** (2498 branch points,
literally equal child counts, median TV against uniform 0.000), so any nonzero exponent makes 3889 of
its 3890 branch points worse. Adjacent binary splits in one trace demand opposite extremes
(`tau2_retail`: `s = 0.043` at depth 69, `s = 5.658` at depth 71), and the exponent correlates with
neither depth (rho +0.13, p 0.15) nor out-degree (rho −0.03, p 0.74) — **no schema-available
conditioner identifies it.**
The collision probability sidesteps all of this because it does not ask the tail's shape. It also
recovers the head as a *consequence*: on `qwen_code`'s 4739-way root split, measured head 0.496, the
exponent matching the collision probability puts **0.464** there — against **0.072** under the
document-level 0.9 default. The tail those rank fits argue over is exactly the part cohort decay does
not read.

**One exponent per depth band, and the bands are the census's.** Within a band the collision
probability varies and one exponent matches only its fan-in-weighted mean — which is the quantity
`cohort *= p` accumulates, so it is the right mean to match. Splits are weighted by fan-in for the
same reason the run length and out-degree are: a walker meets a split in proportion to the sessions
arriving at it, and the shared region is numerically dominated by tiny cohorts while the reference
mass sits in a handful of large segments. Whether the law is better conditioned on **out-degree** than
on depth is left measurable rather than assumed — `fit --explain` prints the within-band p10/p50/p90
spread of the collision probability beside the fitted exponent, and a wide spread is the signal to
re-condition.

**Two boundaries are stated rather than fitted past.** A split no more concentrated than uniform
descent is emitted as uniform with a note, because no Zipf is flatter than uniform and the case is
real: `ragbench`'s deep splits sit at 0.95x uniform, i.e. **sub-multinomial** — more even than random
assignment. A split more concentrated than the widest exponent the fit will state is clamped and
reported, because beyond roughly `s = 8` a two-way split already sends 99.6% of a cohort one way and
the exponent stops being identifiable. A segment ending in a **leaf or in attrition** states no law:
that cohort shrank rather than divided, so there was no choice, and an absent law correctly defers to
the document-level `branch_skew`.

**What this fixes, and what it does not — measured 2026-08-17, seed 4242.** It closes a *pair*
defect. The node-level spelling fitted `out_degree` from the census while the law choosing among those
children stayed at the document default, so on `qwen_code` the model built all 4739 root children and
put 0.072 on a head the trace gives 0.496; sessions scattered ~9x more than the trace and sharing
collapsed. Fitting the law recovers most of that: `qwen_code` `sharing_depth` **0.364 → 0.107** and
`unique_keys` **0.697 → 0.479**. On `tau2_airline` it is a **no-op** (0.371 → 0.376, 0.557 → 0.556),
and the reason is visible in the fit itself — airline's fitted exponents are 0.98–1.56, near the 0.9
default they replace, so the child law was never airline's problem.

Both remain worse than the per-depth profile's 0.060/0.182 and 0.102/0.335, so `--branching-segments`
stays off by default. The residual is that the synthetic mints **1.6–1.7x too many distinct keys**
(`qwen_code` 8.40M against 5.20M; airline 445k against 255k) while per-split collision now matches its
target to **0.4%** in every band — so the cohort divides too often rather than too widely.

#### What the fitted bands say once they are printed — and one wrong diagnosis corrected

The fitted `length` and `out_degree` reached only the emitted YAML, which FR-057 refuses to write
whenever the fit does not resemble its source, i.e. exactly when someone is diagnosing it. They are now
printed under `fit --explain` beside the derived split rate and the cohort decay, and the first thing
that fell out was a **correction**.

**A split rate read off a median is wrong by orders of magnitude, and this note previously made that
error.** The candidate mechanism recorded here was "airline's median segment length is 1 below depth
32, so a walker splits at nearly every block". The number of splits over a depth is a **renewal rate**,
set by the **mean**, and these length distributions are heavily skewed: airline's fitted means are
246 / 354 / 9 / 26 / 71 / 131 blocks, so `splits/blk` is **0.003–0.111**, not ~1. The censuses's `len_med`
of 1 sits under a p90 of 161 in the same band. Nothing about the corpus changed; the reading was wrong.

What the composition does show is sharper, and it **separates the two traces**. Cumulative expected
cohort factor from depth 0, `Π coll^(span/mean len)` per band:

| band | airline decay/band | airline cum | qwen_code decay/band | qwen_code cum |
| --- | --- | --- | --- | --- |
| 0 | 0.99550 | 0.99550 | 0.93401 | 0.93401 |
| 1–7 | 0.98111 | 0.97669 | 0.63782 | 0.59573 |
| 8–31 | 0.18594 | 0.18160 | 0.85987 | 0.51225 |
| 32–127 | 0.05257 | 0.00955 | 0.61441 | 0.31473 |
| 128–511 | 0.00003 | **<1e-6** | 0.24144 | **0.07599** |

**`qwen_code` composes faithfully; airline annihilates its cohort.** qwen_code's dominant root carries
16045 sessions, so a cumulative 0.076 leaves ~1219 still together at depth 511 — against a measured
maximum fan-in of **1127** in that band. Airline's widest root holds 154, so <1e-6 leaves nothing
sharing beyond roughly depth 100–130, while the trace still has fan-in 3–4 out to depth 1200. Realised
sharing p50 is 124 against the trace's 288. This is the same trace on which the child-law fit was a
no-op, and it is why airline stays at 0.376.

**The candidate cause was a small-sample fan-in weighting.** Airline's bands are fitted from **13–36
splits** each, against qwen_code's 104–4067, and airline's fan-in-weighted `deg_mean` comes out
**18.5 / 11.2 / 2.4 / 14.1 / 18.9 / 6.6** where its per-segment census median is **4 / 2 / 2 / 3 / 3 /
4**. At depths 128–511 the fit therefore states an effective branching of `1/0.1405 ≈ 7.1` where the
typical split is 2–3 ways, which reads as a couple of wide, high-fan-in segments setting the band's law.

#### The per-band sample floor is REFUTED — measured 2026-08-18, do not rebuild it

The right measure of how well-observed a *weighted* mean is, is not the split count but **Kish's
effective sample size** `(Σw)²/Σw²` — the same inverse-participation functional as `n_eff` and
`collision`, one level up: there over children, here over the splits a band averages. It is now
reported per band as `ess`, and it **refutes the hypothesis outright**:

| band | airline splits / ess | qwen_code splits / ess |
| --- | --- | --- |
| 0 | 26 / 15.8 | 104 / **2.4** |
| 1–7 | 13 / 4.9 | 2033 / 6.6 |
| 8–31 | 20 / 15.1 | 1137 / 5.8 |
| 32–127 | 28 / 7.3 | 619 / 4.5 |
| 128–511 | 36 / **12.5** | 1334 / 36.3 |
| 512+ | 35 / 23.0 | 4067 / 175.3 |

Airline's cohort-annihilating band has `ess` **12.5**, one of its better-observed; qwen_code's root band,
in the trace that composes *faithfully*, has **2.4** — the worst in either trace. The quantity does not
separate the two traces, and it runs the wrong way in four of six bands. So airline's `coll_wt` of 0.139
against a median of 0.333 is **not** a one-segment artefact: there genuinely are wide, high-fan-in splits
in its deep region, and the weighted mean is a faithful summary of them.

Built anyway and measured, because a prediction is not a measurement. Dropping every band below a floor
and letting the band above carry forward:

| arm | airline share / uniq / reuse | qwen_code share / uniq / reuse |
| --- | --- | --- |
| six bands (committed) | 0.376 / 0.556 / 0.031 | 0.107 / 0.479 / 0.106 |
| ess floor 8 | 0.384 / 0.549 / 0.031 | **0.093 / 0.244 / 0.019** |
| ess floor 13 | 0.431 / 0.551 / 0.031 | — |
| ess floor 20 | 0.533 / 0.589 / 0.032 | 0.093 / 0.244 / 0.019 |
| one pooled band | **0.323 / 0.498 / 0.030** | 0.104 / 0.469 / 0.088 |
| per-depth profile (default) | 0.102 / 0.335 / 0.026 | 0.060 / 0.182 / 0.035 |

Airline gets **monotonically worse** with the floor. qwen_code improves a lot — reuse 0.106 → **0.0188**,
inside its 0.02 tolerance for the first time in this spelling — and floors 8 and 20 give *identical*
output, which is the tell: both leave the same two bands, so the win is not "better-sampled laws" but
the depth-128–511 band's unusually gentle law (`coll` 0.8132, `n_eff` 1.23) being rebased onto depth 0.
**The one-pooled-band control settles it**: if the win were "one law applied globally" the pooled fit
would reproduce it, and it does not (0.0878 / 0.4685 against 0.0188 / 0.2437). The floor is a lottery
over which band's law becomes global, and its direction is arbitrary with respect to its own
justification. Neither toggle is kept; `ess` is, because it is what did the refuting.

**Two things worth keeping from the sweep.** First, on two traces **one pooled band beat six**
(airline 0.376 → 0.323, qwen_code 0.107 → 0.104, `unique_keys` better on both), which looked like a
real lead — the depth banding being mildly counterproductive is consistent with the exponent
correlating with neither depth (rho +0.13, p 0.15) nor out-degree.
**Run across the corpus it did NOT generalise, and the toggle was deleted.** Coverage is 8 of 24 in
all three arms with no fit↔refuse change and nothing inside tolerance anywhere; means over the eight
fitting traces:

| arm | `sharing_depth` | `unique_keys` | `request_length` |
| --- | --- | --- | --- |
| per-depth profile (default) | **0.217** | **0.578** | **0.090** |
| segments, six bands | 0.347 | 0.703 | 0.123 |
| segments, one pooled band | 0.329 | 0.718 | 0.107 |

One band is better on `sharing_depth` and `request_length` but worse on `unique_keys` and on reuse
distance (`browsecompplus` 0.038 → 0.072, `swebench` 0.044 → 0.073) — mixed, so a two-trace result
did not survive twenty-four. **This is what FR-055f exists to catch, and it caught it.** The default
also beats both arms on every mean, confirming corpus-wide what two traces showed.

**A CORRECTION, since this branch asserted the claim six times.** "No trunk work will ever move
`request_length`" is false as stated. It is identical across all three arms on six of the eight
traces, but `browsecompplus` moves 0.083 → 0.168 → 0.136 and `swebench` 0.024 → 0.201 → 0.107.
FR-014a's path length has no trunk *term*, but FR-012a makes the drawn `shared_depth` an **upper
bound** on the realised one, so where the trunk runs out of depth before `shared_depth` is reached the
path really does shorten. The claim holds only while the trunk does not bind, and two traces where it
binds are now known.
Second, and more diagnostic: **the configuration that does best is the least divisive one.** qwen_code's
best arm is the one whose per-split collision is 0.81 — barely dividing at all — and it is *still* short
of the per-depth profile. Every arm over-divides. That is the signature of applying a marginal per-split
law independently at each step when the trace's splits along a path are **negatively correlated**: a
session that passes a wide split lands in a narrow subtree, so the trace's product along a path far
exceeds the product of marginals. Which is, once again, **survival correlates with depth** — the same
mechanism as the refuted `fanout < 1` experiment and the survivor-conditioning result. The next thing to
build is a law conditioned on that correlation, not a better summary of the marginal.

Note also that this is the third independent appearance of one mechanism: **survival correlates with
depth**, so applying a population-average decay to every walker sheds the wrong sessions. It was first
measured in the refuted `fanout < 1` experiment, then again when survivor-conditioning recovered almost
nothing, and now here — and the sample-floor sweep below makes it a fourth, by yet another route.

### Cohort exhaustion as the trunk boundary — step 3, measured and not yet adopted

The named defect behind `sharing_depth` and reuse distance failing 8 of 8 is that the model draws
**independently** what the trace has **correlated**: survival correlates with depth, measured four
independent ways. The mechanism for it is already in the walk and was already right — `plan::generate`
carries `cohort *= p(child taken)` where `p` is the probability of the child *actually drawn*, so a
session that takes a popular child stays in a large cohort and survives deeper while one that takes a
rare child is alone immediately. That is the correlation, and it is per-walker rather than a
population average.

What was missing is only that it was **non-binding**: the walk also tested `d < shared_depth`, and on
any fitted document the cap bound first.

**The isolation that made this testable.** `shared_depth` is doubly loaded — `depth_at_turn` makes
turn-1 depth `shared_depth + private_depth`, so the field is a path-length term as well as the
boundary — and an earlier attempt to remove the boundary by emitting a non-binding *value* inflated
every path 3.7x. Dropping the cap **inside the walk** instead leaves the loop bound (the already-drawn
total depth) untouched, so only the trunk/private boundary moves. Measured, the reference count and
`request_length` come out **bit-identical** in both arms, which is what makes the comparison a
measurement of the mechanism rather than of path length.

| trace | arm | reuse | `sharing_depth` | `unique_keys` |
| --- | --- | --- | --- | --- |
| `tau2_airline` | drawn cap | 0.03101 | 0.37642 | 0.55603 |
| `tau2_airline` | cohort exhaustion | **0.02845** | **0.30570** | **0.43385** |
| `qwen_code` | drawn cap | 0.10615 | 0.10723 | 0.47856 |
| `qwen_code` | cohort exhaustion | **0.02473** | 0.40445 | **0.11961** |

**It is a large win on exactly the two statistics step 1 identified as real defects.** On `qwen_code`
reuse distance improves **4.3x** to 0.0247 — against a floor of 0.0026 and a tolerance of 0.02, so it
is now close to passing — and `unique_keys` improves **4x** to 0.1196, which is **inside** its 0.15
tolerance. All three improve on `tau2_airline`.

**And it makes `sharing_depth` a direct readout of the fitted division rate, which the cap was
masking.** `qwen_code`'s sharing worsens to 0.404, and the reason is mechanical rather than mysterious:
its dominant root holds 16 045 sessions, and with the cap gone a cohort that large takes many splits to
fall below two, so sessions stay shared far deeper than the trace's realised p50 of 7. The fitted
per-split division is too slow — `coll` 0.39-0.81 with mean run lengths of 13-92 blocks. So the
over-division question that § The child-choice law left open is now **directly coupled to a gated
statistic** instead of being hidden behind a drawn cap, which is the useful part of this result.

#### Where the sharing distribution diverges, and the singleton escape it motivated

`--explain`'s CDF table locates it in one glance. The trace's realised sharing is **bimodal with
atoms at 1 and 7 blocks**: 24.8% of requests share one block or less and another 26.5% share exactly
seven, so 57.2% are at or below depth 7. The synthetic under cohort exhaustion produces **1.3%** at or
below one block and 21.7% by seven. So the model badly under-produces sessions that leave the trunk
*immediately*.

That is a mass-allocation problem in the child law's **tail**, and it is a correction to FR-055j.
That requirement fits the law to the collision probability and argues the tail it ignores "does not
affect cohort decay" — true while a drawn `shared_depth` bounded the trunk, **false** once cohort
exhaustion does, because the tail is exactly where sessions go private. A Zipf that matches
`qwen_code`'s head collision probability spreads the remaining mass over ranks 2-118 with enough
weight each to keep those sessions in cohorts the trace has already scattered.

So the census's **singleton share** — the fan-in-weighted fraction of arrivals at a split landing on a
child no other session takes, `(Σc − Σc·[c≥2])/Σc` — was fitted per band and given to the walk as an
escape probability. **The measurement is right**: on `qwen_code` band 0 it comes out **0.2216**
against the trace's 24.8% at or below one block, which is close agreement from an independent route.

**The composition is what fails.** Applied at every split a walker meets, over a path of ~700 blocks
with 0.01-0.07 splits per block, the escape compounds until almost every session has left early:

| `qwen_code` | reuse | `sharing_depth` | `unique_keys` |
| --- | --- | --- | --- |
| cohort exhaustion | **0.02473** | 0.40445 | **0.11961** |
| + singleton escape | 0.12963 | **0.28469** | 0.58196 |

It buys `sharing_depth` and pays 5x on reuse distance and 5x on `unique_keys`, which is a net loss on
two of three. `tau2_airline` loses on all three. So the escape is fitted only under
`CERTUS_EXP_SINGLETON_ESCAPE=1` and no fitted document carries it by default.

**What this narrows the next step to.** The escape magnitude is right at the first split and wrong as
a per-split hazard, which says the quantity the trace has is closer to a **per-session** escape — a
session either lands in the shared spine or it does not — than to an independent coin at every split.
That is the same class of error as the one this whole step exists to fix: an independent draw standing
in for a correlated structure. Do not tune the per-split value; change what the draw is per.

**Not adopted.** Cohort exhaustion makes one gated statistic substantially worse on one trace, and by
the same rule now enforced on the fit's own iteration loop, a trade between two gated statistics is not
an improvement. The next step is calibrating the division rate so that realised sharing lands right, with
`sharing_depth` as the readout — not another marginal fix, since the mechanism is now doing the work
and only its rate is wrong. Reproduce with `CERTUS_EXP_COHORT_BOUNDARY=1` and `--branching-segments`.

### The achievable floor — the derivation behind FR-057c, and what it says about the gate

The tolerances of FR-057b were calibrated from the generator against itself across seeds. That is a
measure of **repeatability**: a bias the model shares with itself cancels exactly, so the numbers
cannot say what a *correct* model of a real workload would score. Six sessions of fitting were
nonetheless judged against them. This section supplies the missing half.

**Method.** `certus-trace floor` splits one real trace into two samples of itself and compares them
with the same accumulators `fit` uses, over the same `Trace::refs_of` — so a half of a trace is
measured by the rules a whole one is, and the halves are asserted to be an exact partition of the
reference stream (a splitter that dropped references would report a *tighter* floor, which is the
dangerous direction). Two splits, because each has a confound and they point opposite ways:

- **by session**, on a mixed hash of the session id: preserves duration and stationarity, but
  **halves the concurrent population**, and sharing is a population property, so both halves
  genuinely share less than the whole. Reads sharing and reuse low.
- **by time**, at the median request timestamp: preserves density, but charges real
  **nonstationarity** to the floor — measured to be strong here (a key's first-to-last span over its
  stationary null is 0.13 on `tau2_airline`).

`request_length` is the one statistic with **no population term** — path length is per request — so
its session-split floor is clean sampling noise plus session heterogeneity, and needs no caveat.
A half-vs-half comparison is half-size on both sides, so a two-sample KS distance, which scales as
`sqrt(2/n)`, is inflated by about `sqrt(2)`; the projection is applied to the KS statistics only,
never to the area or log-ratio measures.

**Measured, session split, nine traces** (projection to full size in brackets):

| trace | reuse (tol 0.02) | share (tol 0.05) | req_len (tol 0.02) | uniq (tol 0.15) |
| --- | --- | --- | --- | --- |
| tau2_airline | 0.0071 | 0.0337 (0.0238) | **0.0425 (0.0301)** | **0.3512** |
| tau2_retail | 0.0076 | 0.0368 (0.0260) | 0.0195 (0.0138) | 0.1120 |
| tau2_telecom | 0.0023 | 0.0201 (0.0142) | 0.0150 (0.0106) | 0.0320 |
| swebench | 0.0026 | 0.0239 (0.0169) | 0.0277 (0.0196) | **0.2001** |
| browsecompplus | 0.0050 | 0.0378 (0.0268) | 0.0153 (0.0108) | 0.0370 |
| qwen_code | 0.0026 | 0.0123 (0.0087) | 0.0089 (0.0063) | 0.0688 |
| qwen_reasoning | 0.0124 | 0.0278 (0.0197) | 0.0127 (0.0090) | **0.2253** |
| ragbench | 0.0021 | 0.0046 (0.0032) | 0.0086 (0.0061) | 0.0234 |

Bold entries are floors **above** their own tolerance. Note that `ragbench` cannot be *fitted* at all
(it supplies no `think_time`) yet its floor measures fine, so this calibration covers the whole
corpus rather than only the traces the model happens to fit.

**Three findings, in order of how much they change what to work on.**

1. **Two of the residuals chased hardest were below their floors.** `request_length` on
   `tau2_airline`: floor 0.030 projected, tolerance 0.020, fitted model **0.026**. `unique_keys` on
   the same trace: floor 0.351, model 0.335. Neither was ever a failure. FR-054c/d/e/f/h were all
   path-length work, and on this trace they were aimed inside the noise.
2. **Reuse distance is the opposite case and is the real defect.** Its floor is 0.002-0.012 against a
   0.02 tolerance, so the tolerance is comfortably reachable, and measured failures of 0.024-0.106
   sit at 3-30x the floor.
3. **The sibling bound is a property of the PAIR, not of the statistic — and reading it from one pair
   is an error this project has already made once.** Comparing two *different* workloads bounds the
   same question from above, and **sibling ÷ own floor** is the dynamic range. Measured over three
   pairs:

   | pair | reuse | share | req_len | uniq |
   | --- | --- | --- | --- | --- |
   | `tau2_airline` / `tau2_retail` | 1.67x | 4.86x | **0.80x** | 1.66x |
   | `swebench` / `browsecompplus` | 33.0x | 12.3x | 10.3x | 12.6x |
   | `qwen_code` / `qwen_reasoning` | 30.3x | 33.6x | 28.4x | 9.9x |

   **The first row was measured first and generalised too soon.** On the strength of it this section
   said "three of the four statistics barely discriminate" and FR-057c said a statistic with range
   below 1 must not be gated on at all, naming `request_length`. Two more pairs refute that: every
   statistic has ample range on both, `request_length` included at 10-28x. **The low numbers are a
   property of how tight the `tau2` pair is** — two task domains of one benchmark harness with one
   agent scaffold, which `corpus_matrix.py`'s own docstring already calls "near-siblings, not three
   independent workloads". On that pair airline and retail genuinely do have near-identical
   request-length distributions, closer than two halves of airline are; that is a fact about those
   workloads, not a defect in the measure. **No statistic is retired on this evidence, and FR-057c was
   corrected.** This is the same error as FR-054g — a claim stated wider than its evidence from the
   tau2 family — which is precisely what the whole-corpus check exists to prevent, so it is recorded
   rather than quietly fixed.
   What the tight pair does establish is a **conservative** bound: a gate required to tell a trace
   from its nearest neighbour in the corpus cannot lean on `request_length` to do it. And
   `reuse_distance_objects` on that pair has a usable band only 0.007-0.012 wide, so its 0.02
   tolerance sits above the whole band there.

**The rule this establishes, and it is the reusable part:** a statistic worth gating on needs **both**
a low floor and a high sibling bound, the bound measured over **several pairs** and reported with the
pair named. The **floor** result is the robust half and is unaffected by the correction above — it is
measured within one trace and needs no second workload.

### Fan-in per block — the FR-057c criterion applied to a candidate (FR-056b)

The point of measuring a floor and a sibling bound is to choose what to gate on. **Fan-in per block**
— distinct sessions referencing a key — is the first candidate, because it is what a lifetime-hinted
cache consumes and because nothing in the model fits it. Applying the criterion rather than the
argument:

| pair | fan-in floor | sibling bound | dynamic range |
| --- | --- | --- | --- |
| `tau2_airline` / `tau2_retail` | 0.00854 | 0.02258 | **2.65x** |
| `swebench` / `browsecompplus` | 0.00113 | 0.00355 | 3.13x |
| `qwen_code` / `qwen_reasoning` | 0.00164 | 0.04634 | 28.28x |

Three pairs, not one — the correction recorded above. Ranked by **worst-case** range across pairs,
which is the right summary for a gate that has to work on every pair rather than on a favourable one:

| statistic | worst-case range | floor range |
| --- | --- | --- |
| `sharing_depth` | 4.86x | 0.005-0.038 |
| **fan-in per block** | **2.65x** | **0.0011-0.0085** |
| `reuse_distance_objects` | 1.67x | 0.002-0.012 |
| `unique_keys` | 1.66x | 0.023-0.351 |
| `request_length` | 0.80x | 0.009-0.043 |

So fan-in is **second of five**, and its floor is an order of magnitude below `sharing_depth`'s, which
means it can carry a much tighter tolerance than any incumbent. It qualifies.

**Exactness has a precondition, and it is checked rather than trusted.** Counting distinct sessions
per key without a set per key means keeping `(count, last_session)`, which is exact only if each
session's references arrive contiguously — `A, B, A` otherwise counts three. That is the same
constraint `fit::segments::Census` carries, and the same remedy: `fit` groups by session while a
streaming accumulator sees an interleaved stream. `FanIn` therefore counts and reports
non-contiguous session reappearances, because the failure biases fan-in **upward** — a workload looks
more shareable than it is, which is the direction that flatters the model rather than exposing it.

**Not gated on yet.** Gating needs the same measurement on the generated plan, and plan events are
not session-contiguous either, so that is a separate piece of work rather than a flag flip.

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

### Reuse-distance estimation, and the basis for `repeat: 8`

Two derivations `spec.md` assigns here, both about when a measurement is affordable and when it means
anything.

#### When exact reuse distance stops fitting, and what replaces it

Exact distances come from a Fenwick tree over stream positions (`stats::reuse_distance`), which is
`O(log n)` per reference and `O(n)` in memory. Measured with `certus-workload report`, which also
materialises the event vector:

| plan | events | peak RSS | bytes/event |
| --- | --- | --- | --- |
| uniform fixture | 2 400 000 | 190 MB | 79 |
| worked example | 2 128 875 | 169 MB | 79 |

Linear, and decomposable: 40 bytes is the caller's own `Vec<PlanEvent>` from `read_plan`, leaving
**~40 bytes per reference** for the statistics — the position tree, its doubling slack, and the key
table at ~48 bytes per distinct key. A stream that never materialises its events pays only the latter.
Varying entry sizes add the byte-distance tree, 8 bytes per position, since the uniform-size shortcut
no longer applies; both fixtures above have constant `block_bytes` and so pay nothing for it.

At a 16 GB budget that caps exact computation at roughly **4 × 10⁸ references** streaming, or 2 × 10⁸
with the events resident. Whether that is enough is not hypothetical — it is decided inside this
corpus:

| trace | invocations | references | blocks/request |
| --- | --- | --- | --- |
| `exgentic_appworld` | 48 453 | **251 546 935** | 5 192 |
| `swe_agent` (250 k of 2 115 623) | 250 000 | 187 610 747 | 750 |
| `exgentic_swebench` | 91 768 | 125 818 574 | 1 371 |
| `wildchat` (250 k of 1 960 074) | 250 000 | 13 409 733 | 54 |
| `mooncake_conv` | 12 031 | 288 500 | 24 |

`exgentic_appworld` at 2.5 × 10⁸ references fits, with little room. **`swe_agent` at full length does
not**: 750 blocks per request over 2 115 623 invocations is ~1.6 × 10⁹ references, about 64 GB. So an
estimator is needed for the tail of this corpus rather than for some hypothetical future trace, and
the surprise is where the cost comes from — not the number of requests, but their length. A trace of
48 000 requests is the most expensive one here.

**The method is hash-based spatial sampling (SHARDS).** Sample the *key space* by hash at rate `R`,
compute exact distances over the sampled subset, and scale each distance by `1/R`. Memory and time
fall by `R`, and the reuse structure survives because every reference to a sampled key is kept.

Sampling *references* instead — the obvious alternative — is wrong rather than merely less accurate:
dropping references breaks the reuse chain, so a surviving pair of references to one key has other
references to that key removed from between them, and the distance between them is measured across a
stream that no consumer would ever have seen. The bias is one-directional and does not average out.

**The exact implementation is what validates the estimator, and this corpus can do it.**
`exgentic_appworld` is computable both ways, so the estimator's error is measurable on real data at
the scale it will be used, instead of taken from the literature. That check belongs with the
estimator; until one is written, `fit` is exact and must refuse a trace it cannot hold rather than
silently sample it.

#### Why `repeat: 8`

FR-046 requires per-point n, mean, cv, a confidence interval and a pairwise verdict at p < 0.05, with
`repeat` defaulting to 8. For a two-sample t-test at α = 0.05 two-sided and 80% power, the sample per
arm is `n = 2(z₀.₉₇₅ + z₀.₈)² / d² = 15.68 / d²`, so **n = 8 detects a standardised effect of d = 1.40**
and the 95% interval half-width is `t₀.₉₇₅,₇ / sqrt(8) = 0.836` standard deviations.

In relative terms that depends entirely on the run-to-run cv, and this repository has measured its
own:

| condition | measured cv | detectable difference at n = 8 | 95% CI half-width |
| --- | --- | --- | --- |
| requester on the socket its NIC and GPU share | 2% | **2.8%** | ±0.6% |
| requester split across sockets (NIC and GPU apart) | 16% | **22.4%** | ±4.7% |

So `repeat: 8` is a good default *and* insufficient on its own: it resolves a few percent on a
well-placed requester and nothing under 20% on a badly-placed one. The actionable part is that
placement buys an order of magnitude more resolution than any affordable increase in `repeat` —
reaching 2.8% at cv 16% would need n ≈ 400.

Corroborated independently in this repository: a remote-delivery throughput change was first measured
at n = 3, found unresolvable, and re-measured at n = 8, which resolved a 26.6% difference at p < 0.05.
The default was chosen before that measurement and survived it.

**What is still not derived.** The two cv figures come from one platform and one benchmark, so they
calibrate the *interpretation* of `repeat: 8` rather than establishing it; a sweep on other hardware
should re-measure its own cv, which is why FR-046 requires cv to be reported per point rather than
assumed.

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

## The whole-corpus fit matrix (FR-055f, T102a)

`research/corpus_matrix.py` fits every trace in the corpus and tabulates the outcome. It exists
because every model change up to FR-054h was measured on `exgentic_tau2_{airline,retail,telecom}` —
three task domains of **one** benchmark, same harness and same agent scaffold. Reporting "three
traces, three seeds, nine cells" made that read as nine observations; the seeds only quantify
generator noise, so the effective sample was **one workload family**.

Measured 2026-08-13, 24 traces, `--block-bytes 131072 --wss-window 5000 --seed 4242`, divergences
against the FR-056 defaults (`req_len` 0.02, `share` 0.05, `reuse` 0.02, `uniq` 0.15):

| trace | sessions | refs | req_len | share | reuse | uniq |
| --- | --- | --- | --- | --- | --- | --- |
| exgentic_browsecompplus | 1 948 | 0.985 | 0.083 | 0.122 | 0.090 | 0.689 |
| exgentic_swebench | 1 959 | 1.002 | 0.036 | 0.203 | 0.093 | 0.154 |
| exgentic_tau2_airline | 957 | 1.027 | 0.026 | 0.073 | 0.024 | 0.219 |
| exgentic_tau2_retail | 1 848 | 0.966 | 0.062 | 0.114 | 0.040 | 0.513 |
| exgentic_tau2_telecom | 1 844 | 0.990 | 0.159 | 0.199 | 0.033 | 0.746 |
| qwen_code | 26 406 | 1.166 | 0.131 | 0.094 | 0.062 | **0.103** |
| qwen_reasoning | 9 612 | 1.006 | 0.139 | 0.133 | 0.030 | 0.532 |
| qwen_toc | 23 101 | 1.006 | 0.024 | **0.028** | 0.029 | **0.079** |
| wildchat | 826 319 | 1.059 | 0.082 | 0.111 | 0.024 | 0.353 |

**Only 9 of 24 traces fit and compare, and within tolerance: `request_length` 0/9, `sharing_depth`
1/9, `reuse_distance_objects` 0/9, `unique_keys` 2/9.** The reference *count* is within ±3.5% on 7 of
9, so volume is calibrated while the **shapes** are not — and the `tau2` traces are the optimistic
end of this table rather than typical of it. Read with FR-054a: each of these is a limitation of the
model, and the table is the honest statement of how much of the corpus it can currently express.

The 15 refusals, by FR-054b classification rather than dropped, because a MODEL LIMITATION that
becomes a fit is progress and a fit that becomes a refusal is a regression: 7 `metadata_only` sources
carry no block data; `mooncake_agent` and `mooncake_conv` carry no session identity;
`exgentic_appworld` has 1 747 requests that shared nothing at all against `shared_depth`'s support
starting at 1; `qwen_tob`, `ragbench`, `ragbench_canonical` and `swe_agent` cannot supply
**`think_time`** — all four the same parameter, measured, none of them the arrival-rate variant of
that refusal (their timestamps are absent or unusable, § Threats to validity item 3); and
`paper_review_dag` is CALLER INPUT, carrying four blockings with no default.

Two errors this sweep found on its first run, both of them **labels rather than numbers**, which is
the more damaging kind because a bad number invites another measurement while a bad label redirects
the work:

- **FR-054g had been stated wider than its evidence.** The reuse-distance U-shape with a floor near
  1.18× the reference count holds on `tau2`; `browsecompplus` and `swebench` sit at reuse 0.090 and
  0.093 — 4.5× the tolerance — with their reference counts already at 0.985 and 1.002. Volume is not
  their problem, so raising it cannot be their fix. The requirement is now scoped to that family.
- **Seven traces were misclassified as CALLER INPUT.** Every `metadata_only` source reported "several
  blockings — `[]` tokens — name one with `--block-size`", and following that advice produced a bare
  `No such file or directory`. An empty list of blockings is not ambiguity. Note the boundary: an
  absent default is *not* the discriminator, since `paper_review_dag` also declares none but lists
  four, and must still be refused as CALLER INPUT.

The sweep then found a third of the same kind: `FitError::{Unmeasured, NoArrivalRate}` named no
FR-054b outcome at all, so the four traces hitting them were reported as **`OK`** — a refusal reading
as success. Both are case 2 by the requirement's own words, a parameter whose source field the trace
does not carry being the model requiring something the trace was never obliged to record.

The exposure that remains, and the reason this is a standing requirement rather than a one-off audit,
is not in any single parameter — every parameter is fitted — but in **prioritisation**: which residual
gets chased is set by whichever traces are in front of whoever is looking, and on a differently-shaped
workload the dominant term may be a different one entirely.

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
- ~~**The `branch_skew` parameterisation**~~ — **discharged 2026-08-17** by § The child-choice law.
  The law is fitted to the one functional the cohort mechanism reads, the collision probability
  `Σ p²`, by bisecting its closed form `H_n(2s)/H_n(s)²`; the rank curve is *not* fitted, and the
  measurement showing no rank law transfers across the corpus is recorded there. Two choices remain
  named rather than derived: the depth banding is the census's rather than a fit to out-degree, and the
  `s = 8` ceiling is where the exponent stops being identifiable rather than a bound on the data.
  The fitting procedures for `shared_depth` and `roots.popularity` remain open.
- ~~**Reuse-distance estimation method** and the **significance-testing approach** behind
  `repeat: 8`~~ — **discharged 2026-08-11** by § Reuse-distance estimation, and the basis for
  `repeat: 8`. Exact costs ~40 bytes per reference, capping at ~4 × 10⁸ on a 16 GB budget, which one
  trace in this corpus already exceeds at full length; SHARDS is the method beyond, validated against
  exact on the largest trace that fits both. `repeat: 8` detects a standardised effect of 1.40, which
  is 2.8% at the measured cv of 2% and 22.4% at 16%.
*(Discharged by removal: the `GetIoStats` cross-check tolerance was the seventh item here. FR-042a,
FR-042b and SC-007a are out of scope — reconciling per-class byte totals against a drive-aggregated
counter requires bounding the consumer's background staging and promotion traffic, which cannot be
done without modelling how that consumer works. See spec § Out of Scope.)

Parked rather than open: an **LP/flow relaxation** for a true offline upper bound under
heterogeneous entry sizes. It was only ever needed to make Belady/OPT a sound ceiling, and OPT is
deferred with the rest of cache simulation (FR-034b).

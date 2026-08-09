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

### Shape taxonomy

Normalised to **tokens**, since block counts are not comparable across traces with different
`block_size`. Medians unless stated.

| Character | Trace | Invocations | Sessions | Turns | Shared prefix | Private tail |
| --- | --- | --- | --- | --- | --- | --- |
| Agentic, tool-heavy | `exgentic_appworld` | 48 453 | 1 500 | 32.3 | 93 088 tok | 208 tok |
| Agentic, long-context | `exgentic_swebench` | 91 768 | 1 959 | 46.8 | 18 272 tok | 288 tok |
| Agentic, transactional | `exgentic_tau2_airline` | 10 916 | 957 | 11.4 | 7 616 tok | 192 tok |
| Agentic, SWE | `swe_agent` | 250 000¹ | 9 661 | 25.9 | 9 136 tok | 240 tok |
| Production code assistant | `qwen_code` | 43 011 | 26 406 | 1.6 | 1 072 tok | 768 tok |
| Production chat | `qwen_toc` | 43 058 | 23 101 | 1.9 | 720 tok | 352 tok |
| Production chat | `wildchat` | 249 216¹ | 86 975 | 2.9 | 128 tok | 160 tok |
| Retrieval / RAG | `ragbench` | 67 351 | 67 351 | 1.0 | 384 tok | 16 tok |
| Pre-hashed conversational | `mooncake_conv` | 12 031 | —² | —² | 512 tok | 2 560 tok |

¹ capped by the probe, not the trace's true length.
² `session_id` is null, so sessions cannot be recovered and `turns` is unfittable.

**Two regimes, ~60× apart in shared-prefix length.** Agentic traces are sharing-dominated (7.6k–93k
tokens shared against a 200–290 token private tail); chat is private-dominated (128–1072 shared
against 160–768 private). A model fitted only on one misses the other entirely, which is why
`spec.md` treats coverage of both as a test-matrix concern rather than picking a single fit target.

**Block-size invariance check.** `exgentic_tau2_airline` measures 59 blocks shared at `block_size`
128 (= 7 552 tokens) and 476 blocks at `block_size` 16 (= 7 616 tokens) — agreement to 0.8%. This is
evidence the delta reconstruction is correct, since an error in it would not cancel across two
blockings of the same source.

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
4. **Truncation.** `wildchat` and `swe_agent` were capped at 120k–250k invocations, so their session
   counts and width profiles are lower bounds.
5. **The fanout threshold is arbitrary.** 1.8× was chosen to make the structure visible, not
   derived. The segmentation rule is an open derivation below, and the flat-run evidence does not
   depend on the threshold.

## Open derivations

Assigned to this file by `spec.md` and **not yet done**:

- **The trunk-occupancy bound and the `auto` closed form** (FR-009f/FR-009g) — full derivation, and
  the `target_occupancy = 4` choice, which § Width and occupancy by depth supports but does not
  establish.
- **The segmentation rule for fitting a `branching` profile** — what jump ratio counts as a fanout
  event, how to choose segment boundaries robustly when width is noisy, and how the near-root
  boundary of FR-055c interacts with it. § Trunk width is piecewise used 1.8× as a threshold chosen
  by eye; that is not a rule.
- **The four default per-statistic `fit`/`validate` tolerances** (FR-057b), including which
  divergence measure each statistic uses — the four are on different scales, so each needs its own
  measure as well as its own threshold.
- **The `branch_skew` parameterisation**, and the fitting procedures for `shared_depth` and
  `roots.popularity`.
- **Reuse-distance estimation method** and the **significance-testing approach** behind `repeat: 8`.
- **The `GetIoStats` cross-check tolerance** (FR-042b) — how background staging and promotion
  traffic is bounded or subtracted out of a drive-aggregated counter.

Parked rather than open: an **LP/flow relaxation** for a true offline upper bound under
heterogeneous entry sizes. It was only ever needed to make Belady/OPT a sound ceiling, and OPT is
deferred with the rest of cache simulation (FR-034b).

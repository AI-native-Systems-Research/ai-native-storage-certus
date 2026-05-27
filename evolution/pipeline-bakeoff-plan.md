# Plan: Multi-Framework Evolution Bakeoff on Certus Transfer Pipeline

## Context

Presentation showing how evolutionary frameworks optimize Certus and which framework works best. Three stages:

1. **Nous** — Controlled hypothesis (A vs B): Does concurrency change which transfer architecture wins?
2. **Bakeoff** — 6 frameworks optimize the pipeline (same target, two evaluators: fixed-size + mixed-size)
3. **Validation** (if time) — Confirm improvement translates to real TTFT via llm-d serving benchmark

**Why evolutionary frameworks for AI-native LLM inference storage:**
1. **Combinatorial parameter space** — 5+ interacting parameters; manual grid search intractable; non-obvious interactions (larger ring depth helps at low concurrency but hurts at high due to memory pressure)
2. **Workload-dependent optima** — LLM inference creates bursty, size-skewed access patterns; prefill (large sequential KV blocks) vs decode (small random lookups) need different configs
3. **Hardware co-evolution** — New GPU generation changes PCIe bandwidth, BAR1 size, DMA engine count; evolution re-discovers optimum automatically
4. **Beyond parameter tuning** — LLM-guided evolution restructures logic (adaptive chunk sizing, batched sync), not just constants. Unreachable by grid search.

**Primary evaluator:** `certus-api-bench.py` — live gRPC benchmark against certus-server (7 NVMe SSDs, NVIDIA A30 GPU).

---

## Stage 1: Nous — Controlled A vs B (H3: Concurrent Scaling)

Nous requires structured hypotheses with explicit arms and control-negatives. NOT open-ended "improve X" — that's what population tools do.

Nous answers **H3**: "Under concurrent load, is the bottleneck global serialization or device bandwidth?" This is the question population tools CANNOT answer because the fix spans multiple files.

### P2P implementation status (verified 2026-05-25)

**P2P is COMPLETE:**
- `gpu-services/src/dma.rs` (700+ LOC): `create_spdk_dma_buffer_from_gpu_bar`, `create_spdk_dma_buffer_from_phys`, `create_spdk_dma_buffer_from_bar_direct`
- `gpu-services/src/bin/p2p_server.rs`: Full NVMe→GPU P2P server with bounce/p2p/p2p-cold modes
- `apps/gpu-bb-vs-p2p`: Standalone benchmark (3227 vs 3377 MB/s = 5% gap at 1 client)
- Feature gate: `#[cfg(feature = "p2p")]` in gpu-services, enabled with `features = ["p2p"]`

What's needed for Nous: wire the P2P path into `certus-server`'s `promote_and_serve()` so certus-api-bench.py can measure it. This is connecting two existing implementations, not building from scratch.

### H8 (revised): "P2P direct outperforms zero-copy pipeline under high-concurrency load"

**Rationale (from 2026-05-25 baseline):**
- 1 client: 4.9 GB/s (p50 = 550 µs) — pipeline at ZERO_COPY_DEPTH=16, 2 streams
- 4 clients: 7.8 GB/s aggregate — good scaling (1.6× over 1-client)
- 8 clients: 5.8 GB/s aggregate — **REGRESSION** vs 4 clients (26% drop!)

The 8-client regression is the key finding. Two possible causes:
1. **Pipeline Mutex contention** — `self.pipeline_ring.lock().unwrap()` serializes ALL cold lookups through one lock
2. **DRAM bus saturation** — 8 clients × 4 MiB each streaming through host memory = 32 MiB concurrent DRAM traffic

P2P bypasses host DRAM entirely (NVMe → GPU BAR1 via GDRCopy + nvidia-peermem). If cause #2 dominates, P2P will recover the regression. If cause #1, we need per-drive pipeline state.

**Arms:**
- **Arm A (zero-copy pipeline)** — Current default: `pipelined_ssd_to_gpu_zero_copy`. Global PipelineRing Mutex, ZERO_COPY_DEPTH=16, 2 CUDA streams. NVMe → DRAM → GPU.
- **Arm B (P2P direct via GDRCopy)** — Wire existing `create_spdk_dma_buffer_from_gpu_bar` into `promote_and_serve()`. NVMe reads directly to GPU BAR1 pages. Bypasses host DRAM. P2P implementation already complete in `dma.rs` and tested in `p2p_server.rs`.
- **Arm C (control-negative)** — Single synchronous `drive.read()` + single `gpu.dma_copy_to_device()`. No pipelining, no overlap. Proves pipelining itself has value.

**Measurement:** `certus-api-bench.py` at 1, 4, and 8 clients. Key metric: does Arm B recover the 4→8 regression?

**What Nous uniquely answers:**
- Does P2P recover the 8-client regression? → DRAM was the bottleneck, P2P bypasses it
- Does the 5% gap at 1-client (micro-benchmark) widen to >15% at 8 clients? → P2P becomes the default
- Does neither architecture fix it? → The problem is pipeline_ring Mutex contention, need per-drive state
- Is pipelining essential? → Control-negative arm measures pipeline overhead vs benefit

**Multi-file requirement:** Wiring P2P into `promote_and_serve()` = `pipeline.rs` (new P2P pipeline function) + `lib.rs` (call site dispatch) + `Cargo.toml` (enable `p2p` feature for dispatcher). P2P DMA code itself already exists.

**Why this is THE right hypothesis for the presentation:**
1. It addresses a **real production problem** — LLM inference has 48 concurrent prefetch workers. The 4→8 regression means Certus collapses under real workload.
2. It's **actionable** — if P2P wins at concurrency, we ship it as the default path.
3. It **demonstrates Nous's unique value** — no population tool can reason about "why does throughput drop at 8 clients" and test two architectural approaches head-to-head.
4. The **P2P code already exists** — this is connecting working implementations, which is exactly what Nous does (controlled experiments, not open-ended development).

### Alternative hypothesis (if H8 reveals Mutex is the bottleneck):

**H8b: "Per-drive pipeline state eliminates the 8-client regression"**
- Arms: global PipelineRing Mutex (current) vs per-drive pipeline state (each drive has own streams + depth) vs lock-free pipeline (atomic submission queue)
- Metric: Aggregate throughput at 8 clients — does it recover to linear scaling?
- Code target: `lib.rs:promote_and_serve()` — select pipeline ring by drive index instead of global lock
- Why separate from H8: if P2P doesn't fix the regression (i.e., it's Mutex not DRAM), this hypothesis isolates the Mutex as the cause

### Lighter hypothesis (testable by population tools during bakeoff):

**H4: "Sync frequency dominates pipelining benefit at small transfer sizes"**
- Arms: sync-every-16 (current) vs sync-at-end-only vs sync-every-4
- Metric: Cold lookup throughput at 1 MiB (8 chunks) and 4 MiB (32 chunks)
- Code target: `pipeline.rs` line 356 — the `if (completed + 1) % 16 == 0` check
- Can be tested by population tools (single constant change) — include as "warm-up" for bakeoff

---

## Three Hypotheses

| # | Hypothesis | Answered by | Evaluator setup |
|---|---|---|---|
| **H1** | "There exists an optimal pipelining configuration that significantly outperforms current dispatcher and matches or exceeds hardware bypass (P2P)" | Bakeoff (all 6) + Nous (controlled A vs B vs C) | 1 client, 1 drive, 4 MiB. Score = cold throughput GB/s |
| **H2** | "Adaptive pipelining generalizes across object sizes (1-16 MiB) without per-size tuning" | Bakeoff — top 3 from H1 | 1 client, 1 drive, mixed 1/2/4/16 MiB. Score = composite |
| **H3** | "Under concurrent load, pipeline scalability is limited by global serialization, not device bandwidth" | Nous + claude_code | 8 clients, 7 drives, 4 MiB. Score = aggregate throughput |

**Why 3 hypotheses:**
- H1 is the central thesis: evolution finds what humans missed, and pipelining is architecturally correct (no need for P2P hardware complexity). All 6 frameworks search for the optimal config; Nous confirms it causally via controlled A/B against P2P.
- H2 tests whether evolution can discover structural logic (adaptive branching), not just better constants — this is where framework sophistication should matter
- H3 tests a question only multi-file tools can answer (the Mutex is in lib.rs, not pipeline.rs) — makes the case for Nous/claude_code

**Performance reference points (measured 2026-05-25):**
- Raw NVMe → DRAM (iops-benchmark, QD=32, 128K seq): **5.28 GB/s** — absolute device ceiling
- Current pipeline (certus-api-bench, 1 client): **4.9 GB/s** — already 93% of device ceiling!
- P2P direct (gpu-bb-vs-p2p): **3.4 GB/s** — only 64% of ceiling (pipeline already beats P2P by 44%)
- Bounce-buffer micro-bench (gpu-bb-vs-p2p): **3.2 GB/s** — different config than server pipeline
- 4-client aggregate: **7.8 GB/s** — good multi-drive scaling
- 8-client aggregate: **5.8 GB/s** — REGRESSION (26% drop from 4 clients)

**Key insight:** At 1-client, the pipeline is already near-optimal (93% of device ceiling). There's only ~7% headroom left for H1. The REAL opportunity is H3 — the 8-client regression means aggregate throughput collapses to 5.8 GB/s when it should scale to ~35 GB/s (7 drives × 5 GB/s). That's a 6× gap.

**Nous role in H1:** After bakeoff finds the optimal pipeline config, Nous runs a controlled comparison:
- **Arm A (evolved pipeline)** — best candidate from bakeoff
- **Arm B (P2P direct)** — wire existing P2P path into dispatcher
- **Arm C (control-negative)** — synchronous read + single DMA copy (no pipelining)
- This answers: "does optimal pipelining beat P2P?" and "by how much?" — a CAUSAL answer, not just a benchmark number.

**Framework eligibility:**

| Framework | H1 (optimize) | H1 (A vs B) | H2 (adaptive) | H3 (concurrency) |
|---|---|---|---|---|
| AdaEvolve | ✓ | ✗ | ✓ | ✗ (single-file) |
| EvoX | ✓ | ✗ | ✓ | ✗ |
| GEPA | ✓ | ✗ | ✓ | ✗ |
| K-Search | ✓ | ✗ | ✓ | ✗ |
| ShinkaEvolve | ✓ | ✗ | ✓ | ✗ |
| AdaEvolve claude_code | ✓ | ✗ | ✓ | **✓** (multi-file) |
| Nous | ✗ | **✓** (controlled) | ✗ | **✓** (controlled) |

---

## Stage 2: Optimization Bakeoff — 6 Frameworks

All 6 frameworks answer H1 (and top 3 answer H2) on the same target with the same evaluator and budget.

### Frameworks (6 contenders)

| # | Framework | Key Property | Multi-file? | What it CAN discover |
|---|---|---|---|---|
| 1 | **SkyDiscover (AdaEvolve)** | 3-level adaptive hierarchy | No | Better constants, sync cadence, loop structure within pipeline.rs |
| 2 | **SkyDiscover (EvoX)** | Self-evolving search strategy | No | Same as AdaEvolve but may find different solutions via better search |
| 3 | **SkyDiscover (AdaEvolve `claude_code`)** | Full workspace, multi-file | **YES** | Change PipelineRing struct, add streams, modify lib.rs call site |
| 4 | **GEPA** | Reflective Pareto, reads full traces | No | Targeted fixes based on build/bench error diagnostics |
| 5 | **K-Search** | World-model tree, backtracking | No | Deep structural changes via multi-step planning (e.g., try 3 different approaches, backtrack from dead ends) |
| 6 | **ShinkaEvolve** | Async pipeline, fast throughput | No | Volume-based: many variants fast, crossover between good candidates |

### Two Evaluators

Run each framework against **both** evaluators to test generalization:

**Evaluator A: Fixed 4 MiB, single drive (H1)**
- Server started with 1 data drive: `--data-pci 0000:62:00.0`
- `certus-api-bench.py --clients 1 --num-objects 16 --iterations 10`
- Score = cold lookup throughput in GB/s (p50-based)
- Tests: can the pipeline saturate a single NVMe device? Ceiling ~6-7 GB/s.
- Single drive eliminates cross-drive distribution noise — pure pipeline signal

**Evaluator B: Mixed sizes, single drive (H2)**
- Same single-drive server setup
- Run four times: `--block-size 1048576` (1 MiB = 8 chunks) + `--block-size 2097152` (2 MiB = 16 chunks) + `--block-size 4194304` (4 MiB = 32 chunks) + `--block-size 16777216` (16 MiB = 128 chunks)
- Score = 0.25 × 1MiB + 0.25 × 2MiB + 0.25 × 4MiB + 0.25 × 16MiB
- Tests: pipeline that generalizes across KV cache sizes (Llama-8B = 2 MiB, Llama-70B = 5 MiB, factor=16 = 32-80 MiB)
- 16 MiB = 128 chunks: tests sustained throughput at high chunk count (QD and sync amortization matter most here)
- Why this tests H2: a pipeline tuned for 4 MiB (QD=32, sync every 16) will be wrong for 1 MiB (only 8 chunks — QD=32 wastes priming) and 16 MiB (128 chunks — sync every 16 creates 8 pipeline bubbles). Must discover adaptive logic.
- Requires: `--block-size` flag in `certus-api-bench.py` ✓ (done)

**Evaluator C: Concurrent, all drives (H3 — Nous/claude_code only)**
- Server with all 7 data drives (default config)
- `certus-api-bench.py --clients 8 --num-objects 16 --iterations 10`
- Score = aggregate cold throughput GB/s
- Baseline: 5.8 GB/s (should be ~35+ if scaling linearly with 7 drives × ~5 GB/s each)
- Tests: is the bottleneck the global pipeline_ring Mutex or device saturation?

**Why both:**
- Fixed 4 MiB: clean comparison, fast eval (~40s), all frameworks get 30 iterations. At 32 chunks, the pipeline is heavily amortized — tests steady-state throughput optimization.
- Mixed sizes: 1 MiB (8 chunks) is MORE sensitive to pipeline tuning — less opportunity for pipelining to overlap, so depth/sync strategy matters more. Forces evolution to discover adaptive behavior (e.g., "if num_chunks < 12: use depth=num_chunks, sync at end; else: use depth=16, sync every 16"). **This is where evolution shows maximum value** — a human would just tune for one size, evolution can discover size-adaptive strategies.
- Presentation slide: "does the bakeoff winner change when the evaluator changes?" — tests framework robustness

**Budget:** 30 minutes wall-clock per framework per evaluator. Single-file tools: ~30 iterations (Evaluator A) or ~15 iterations (Evaluator B, 2x eval time). `claude_code` mode: ~3-5 iterations regardless.

**Total bakeoff time:** 6 frameworks × 2 evaluators × 30 min = 6 hours. OR: run all 6 with Evaluator A (3 hours), then top 3 with Evaluator B (1.5 hours) = 4.5 hours.

### What each framework targets and its implicit hypothesis

All 6 frameworks optimize the same file (`pipeline.rs` ~407 LOC) but they'll approach it differently based on their search strategy. The question is: does the framework's approach matter for THIS target?

| # | Framework | Implicit hypothesis it tests | What we expect it to find | Why it might fail |
|---|---|---|---|---|
| 1 | **AdaEvolve** | "Adaptive population search finds good constants without tuning" | Higher ZERO_COPY_DEPTH (32-64), better sync cadence | May not escape local optimum of "just increase depth" |
| 2 | **EvoX** | "Self-evolving search strategy outperforms fixed algorithms" | Same constants but faster convergence; possibly discovers strategy switches mid-run | Overhead of meta-evolution not worth it on a 30-iter budget |
| 3 | **AdaEvolve `claude_code`** | "Multi-file edits unlock structural improvements unreachable by single-file tools" | Per-size pipeline selection, PipelineRing restructuring, additional CUDA streams | 10-50× slower — only 3-5 iterations, may not converge |
| 4 | **GEPA** | "Reading build/bench traces enables targeted fixes in fewer iterations" | Targeted: reads compiler error → fixes it; reads bench output → identifies bottleneck | May over-fit to diagnostic signals rather than exploring broadly |
| 5 | **K-Search** | "World-model reasoning + backtracking finds deep structural changes" | Novel pipelining structure (batched completion processing, adaptive depth per chunk count) | Complex reasoning overhead; may reason correctly but implement incorrectly |
| 6 | **ShinkaEvolve** | "Volume-based search with crossover finds good solutions through quantity" | Discovers combinations via crossover between good candidates | Shallow mutations; unlikely to find adaptive logic without explicit guidance |

**The bakeoff answers:** Does framework choice matter? Do expensive frameworks (K-Search, claude_code) find qualitatively different solutions than cheap ones (ShinkaEvolve, AdaEvolve)?

### Search space (what the frameworks CAN change)

Target: `pipeline.rs` (~407 LOC). Parameters the frameworks will discover:
- ZERO_COPY_DEPTH (1-64) — current: 16. Controls max NVMe commands in-flight.
- PIPELINE_RING_SIZE (2-32) — current: 8. Only used by old ring-buffer path.
- Chunk size (64 KiB - 1 MiB) — current: 128 KiB (MDTS). Larger = fewer commands but higher per-command latency.
- Number of CUDA streams (1-4) — current: 2. More streams = more GPU DMA overlap.
- Sync frequency (every N completions vs only at end) — current: every 16. Controls GPU command queue depth.
- Prime count (how many reads to issue before processing completions) — current: min(ZERO_COPY_DEPTH, num_chunks).
- **Adaptive logic** (branch on `total_bytes` to use different strategy for small vs large) — doesn't exist yet. This is where evolution shows value.
- Completion processing order (round-robin vs drain-all-ready) — current: round-robin by stream index.

**Note on QD:** ZERO_COPY_DEPTH=16 is the current queue depth. Frameworks will almost certainly try higher values (32, 64). At 16 MiB = 128 chunks, QD=32 or 64 likely helps. We don't seed this — let the frameworks discover it. If ALL frameworks find QD=32 as their first improvement, it proves the search space has obvious wins (good for "why evolution works" narrative).

For `claude_code` mode additionally:
- PipelineRing struct changes (variable stream count, different buffer sizing)
- lib.rs promote_and_serve (pass additional context like transfer size)
- Conditional path selection (small transfers skip pipeline entirely)

### Framework Comparison Axes

| Axis | What it measures | How we compute it | Source |
|---|---|---|---|
| **Best score** | Peak performance | max(score) across iterations | AdaEvolve Table 2 |
| **Mean ± Std** | Reliability | Mean/std of top-5 candidates | AdaEvolve Table 2 |
| **Sample efficiency** | Evals to 90% of best | First iteration exceeding 90% threshold | GEPA, K-Search |
| **Convergence speed** | Score vs wall-clock | Accounts for async (ShinkaEvolve) and overhead (claude_code) | HPO literature |
| **Cost** | API spend | Tokens × price per iteration | Practical |
| **Discovery novelty** | Non-obvious structure? | Qualitative | K-Search, GEPA case studies |
| **Stagnation recovery** | Escape local optima | Flat iterations before breakthrough | EvoX, AdaEvolve |
| **Generalization** | Winner on Eval A also wins on Eval B? | Score ratio between evaluators | Novel axis |

---

## Stage 3: Validation — llm-d (if time + improvement > 10%)

If bakeoff produces >10% cold lookup improvement:
- Set up vLLM + CertusOffloadingSpec
- Replay Qwen production trace
- Measure TTFT by turn bucket (turn 1 vs 2-5 vs 6-10 vs 11+)
- Compare baseline pipeline vs evolved pipeline

---

## Implementation Steps

### Step 1: Baseline measurements
- Run `gpu-bb-vs-p2p` parameter sweep (ring depth 4/8/16/32, chunk size 64K/128K/256K/512K)
- Run `certus-api-bench.py` at 1/4/8 clients, 3× each for variance
- Record all numbers for presentation Slide 2

### Step 2: Add `--block-size` flag to certus-api-bench.py
Currently `BLOCK_SIZE = 4 * 1024 * 1024` is hardcoded. Add CLI arg so mixed-size evaluator works.

### Step 3: Nous campaign — H8 revised
Create `evolution/pipeline-bakeoff/nous-campaign.yaml`:
- Research question: "Does P2P direct outperform zero-copy pipeline under concurrent load (4-8 clients)?"
- Arms: zero-copy pipeline vs P2P direct vs control-negative (single sync read)
- Benchmark: `certus-api-bench.py` at 1, 4, 8 clients
- Max iterations: 5-8

### Step 4: Create shared evaluator for bakeoff
`evolution/pipeline-bakeoff/evaluator/evaluate.sh` + `evaluator.py`:
- Receives candidate pipeline.rs
- Patches, builds, restarts server, runs benchmark, returns JSON score
- Two modes: `--eval fixed` (4 MiB only) and `--eval mixed` (128 KiB + 4 MiB composite)

### Step 5: Prepare initial program + campaign configs
- `initial_program.rs` = pipeline.rs with EVOLVE-BLOCK markers
- 6 config files (one per framework)
- Shared system message with domain context

### Step 6: Run bakeoff
- All 6 frameworks × Evaluator A (fixed 4 MiB) = 3 hours
- Top 3 winners × Evaluator B (mixed sizes) = 1.5 hours
- Total: ~4.5 hours

### Step 7: Analyze + validate
- Compute all comparison axes
- Generate convergence plots
- Validate winner at 1/2/4/8 clients
- (If time) llm-d live serving

---

## Presentation Structure

### Slide 1: "Why Evolutionary Frameworks for AI-Native Storage?"
- Combinatorial parameter space, workload-dependent optima, hardware co-evolution
- Key claim: evolution restructures logic, not just tunes constants
- Show the pipeline as example: 5+ interacting parameters, non-linear behavior

### Slide 2: "The Target — Transfer Pipeline"
- Architecture diagram: NVMe → (ring-buffer | zero-copy | P2P) → GPU
- Current performance: 4.9 GB/s (1 client), 7.8 GB/s (4 clients), **5.8 GB/s (8 clients — REGRESSION)**
- The regression tells the story: scaling collapses at concurrency. This is what evolution must fix.
- Hot lookup reference: 16 GB/s (1 client), 20 GB/s (4+ clients) — proves GPU DMA isn't the bottleneck

### Slide 3: "Nous — Diagnosing the 8-Client Regression"
- The problem: aggregate throughput DROPS 26% from 4→8 clients (7.8 → 5.8 GB/s)
- Hypothesis: DRAM bus saturation under concurrent load — P2P (NVMe → GPU direct) bypasses it
- Arms: zero-copy (NVMe→DRAM→GPU) vs P2P (NVMe→GPU BAR1 via GDRCopy) vs control-negative (no pipelining)
- P2P implementation already exists and tested (gpu-bb-vs-p2p showed 5% at 1 client — does the gap widen at 8?)
- Result: what Nous found (DRAM bus or Mutex or device saturation?)
- Key message: "Nous answers WHY before you optimize WHAT — with causal evidence, not guessing"

### Slide 4: "The Bakeoff — 6 Frameworks, 2 Evaluators"
- Framework taxonomy (2×2: Information Channel × Search Topology)
- Why 6: 4 single-file + 1 multi-file variant + 1 async throughput
- Two evaluators: fixed-size (optimize for one point) vs mixed-size (must generalize)
- Fair comparison: same budget, same hardware, same target

### Slide 5: "Comparison Axes"
- Table of 8 axes with paper sources
- Why each matters for production use

### Slide 6: "Results — Fixed Size Evaluator"
- Convergence plot (all 6 on same chart)
- Score table (framework × axis)
- Code diff of winner

### Slide 7: "Results — Mixed Size Evaluator"
- Did the winner change? (If yes: evaluator design matters as much as framework choice)
- Did any framework discover adaptive logic? (branching on transfer size)
- This is where evolution's value is clearest vs manual tuning

### Slide 8: "What Evolution Discovered"
- Qualitative analysis of best candidates
- Human-intuitive changes (parameter tuning) vs surprising discoveries (structural)
- Multi-file winner (if claude_code mode found something others couldn't)

### Slide 9: "Framework Recommendations"
- Decision tree: when to use which framework for storage optimization
- Cost vs performance tradeoff per framework
- "For fast iteration: ShinkaEvolve. For best result: EvoX/K-Search. For architecture decisions: Nous"

### Slide 10: "Future Work"
- Trace replay evaluator (1s eval → 1000 iters/hour)
- Multi-objective Pareto: throughput × tail latency × memory
- Eviction policy evolution (second target)
- Live vLLM serving validation
- Auto-re-evolution on hardware upgrade
- Cross-component co-optimization (Nous H7)

---

## Value Assessment — Where Evolution Brings Real Value

| What evolution can discover | Value vs manual effort | Which frameworks find it |
|---|---|---|
| Better constants (depth=24 instead of 16) | **Low** — human grid search finds this in hours | All frameworks |
| Better sync frequency (sync every 8 instead of every 16) | **Medium** — non-obvious interaction, human needs experimentation | All frameworks |
| Adaptive logic (branch on transfer size) | **High** — human wouldn't try this without prior hypothesis | K-Search, claude_code, GEPA (reflective) |
| Structural changes (3 streams, batched completion) | **High** — requires reasoning about hardware + restructuring code | K-Search, claude_code |
| Novel algorithm (e.g., predictive prefetch based on pattern) | **Very high** — only possible with LLM creativity | K-Search (world model), EvoX (strategy evolution) |

**The presentation's strongest slide will be:** "Here's what evolution discovered that a human wouldn't try" — this is the killer result. If all frameworks only find parameter tuning, the presentation is weaker. The mixed-size evaluator is specifically designed to force this: you can't win on both 1 MiB AND 4 MiB by just changing a constant — you need adaptive logic (e.g., "if num_chunks < 12: skip intermediate sync, use full depth = num_chunks").

---

## Evaluator Validity — Why certus-api-bench.py Works

**Signal dominance:** The cold lookup path in `promote_and_serve()` is:
1. gRPC overhead (~10-20 µs) — negligible
2. Dispatch-map lookup + lock acquire — negligible
3. `evict_for_space()` — no-op when memory-tier isn't full
4. `mt.insert()` — memory-tier slot allocation (~1-5 µs)
5. **`pipelined_ssd_to_gpu_zero_copy()`** — 32 NVMe reads + 32 GPU DMAs — **>95% of latency**
6. Dispatch-map update — negligible

At 573 µs per 4 MiB cold lookup (7.3 GB/s), non-pipeline overhead is <30 µs. The pipeline IS the measurement.

**Headroom exists:** 4.9 GB/s (1 client) vs ~25 GB/s PCIe Gen4 theoretical = 20% utilization. But hot lookups (DRAM→GPU only) achieve 15-16 GB/s at 1 client — so the NVMe read portion is the bottleneck, not GPU DMA.

**Variance concern (measured 2026-05-25):** Cold lookup avg at 1 client: 4.41 / 5.34 / 5.08 GB/s = **10% CoV** on avg-based score. The p50 is tighter (469-618 µs). The high avg-variance is driven by eviction outliers (p99 up to 3.1 ms).

**Mitigation for bakeoff scoring:**
- Use **p50 cold latency** as primary score (not avg) — eliminates eviction tail noise
- OR: pre-fill memory-tier before measuring (increase --num-objects until pool saturates, then only measure iterations after saturation)
- A framework needs >20% improvement to be confidently distinguished from noise

**Size sensitivity:** Smaller objects (1 MiB = 8 chunks) are MORE sensitive to pipeline tuning than 4 MiB (32 chunks), because less overlap opportunity means depth/sync strategy choices matter more. This makes the mixed-size evaluator MORE discriminating, not less.

**What certus-api-bench.py CANNOT test:**
- Eviction policy quality (need trace replay for that — different target entirely)
- Multi-tenant fairness under concurrent load (partially: `--clients 4` tests aggregate throughput)
- End-to-end TTFT (need llm-d live serving — Stage 3 validation only)

---

## Verification

- `gpu-bb-vs-p2p` binary exists and runs
- P2P path compiles with `p2p` feature + nvidia-peermem loaded (needed for Nous Arm B)
- Server restartable (kill PID, relaunch, gRPC responds within 5s)
- `certus-api-bench.py` with `--block-size` flag works at 128 KiB and 4 MiB
- Baseline variance <5% across 3 runs
- Evaluator script: builds, restarts, benchmarks, returns JSON correctly
- Each framework succeeds for at least 3 iterations before full run
- Nous: P2P arm compiles and produces valid benchmark results

## Key Files

| File | Role |
|---|---|
| `components/dispatcher/src/pipeline.rs` | Primary evolution target (~407 LOC) |
| `components/dispatcher/src/lib.rs` | promote_and_serve call site (Nous multi-file + claude_code) |
| `components/gpu-services/src/dma.rs` | P2P path (Nous Arm B) |
| `components/memory-tier/src/lru.rs` | Eviction target (future work) |
| `apps/python/certus-api-bench.py` | Primary evaluator (needs --block-size flag) |
| `evolution/evolution_strategy.md` | Framework taxonomy reference |
| `evo_frameworks/skydiscover/` | AdaEvolve + EvoX + claude_code modes |
| `evo_frameworks/gepa/` | GEPA |
| `evo_frameworks/K-Search/` | K-Search |
| `evo_frameworks/ShinkaEvolve/` | ShinkaEvolve |
| `evo_frameworks/agentic-strategy-evolution/` | Nous |
| `evolution/pipeline-bakeoff/` | Configs + evaluator + results (to create) |

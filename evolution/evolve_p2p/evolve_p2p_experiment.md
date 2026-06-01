# Directed Evolution of a Storage-to-GPU Data Path

## Abstract

We evaluate whether LLM-guided evolutionary frameworks can transform a storage-to-GPU
data path, rather than merely tune parameters. The starting system moves data through a
CPU-mediated path: NVMe SSD → host DRAM → GPU. The target behavior is a verified low-CPU
or GPU-direct path, when supported by the hardware and software stack. We treat this as a
directed-evolution problem: a fitness function applies selection pressure using only
observable properties — throughput, tail latency, CPU utilization, host memory bandwidth,
and correctness. The key question is whether different evolutionary frameworks can escape
the local optimum of an optimized CPU-bounce path and discover a coordinated multi-file
architectural change.

We borrow the language of directed evolution (Arnold, 2018) to describe selection pressure,
local optima, mutational operators, and valley crossing. We do not claim a biochemical
equivalence; the analogy is methodological.

The hardware is configured to potentially support a direct or reduced-CPU NVMe→GPU path;
feasibility is established by preflight checks and a manual positive control before
evolution begins. If the positive control cannot be established, the experiment pivots
to measuring how close evolution can get to the reduced-CPU optimum within the bounce
architecture.

---

## 1. Experimental Design

### 1.1 Directed Evolution Framing

| Concept | Biology | This Experiment |
|---------|---------|-----------------|
| Wild-type protein | Current pipeline.rs (bounce path) | Starting code |
| Fitness function | Enzyme activity assay | Composite score: throughput + tail latency, correctness as hard constraint |
| Mutagenesis | Error-prone PCR / saturation mutagenesis | LLM-proposed code mutations |
| Selection pressure | Survival / activity threshold | Score-based acceptance |
| Local optimum | Active but suboptimal fold | Optimized bounce path (~0.60) |
| Global optimum | Optimal catalytic fold | Unknown — determined empirically by calibration |
| Fitness valley | Inactive intermediates | Broken builds, corrupted data |
| Directed evolution round | Screen → select → amplify → mutate | Evaluate → reflect → propose → accept |
| Epistasis | Mutations that only help in combination | Path change requires both buffer allocation AND pipeline rewrite |

### 1.2 Fitness Landscape Hypothesis

We hypothesize a fitness landscape with **multiple candidate optima** — the true global
optimum is determined empirically, not assumed:

```
Score
  ^
  |    ?           ?              ?
  |    *           *              *        ← Unknown global optimum
  |   /|          /|             /|
  |  / |   or   / |    or     / |    or something else
  | /  |       /  |          /  |
  *----+------+---+---------+---+---→ Architecture complexity
  |    |      |   |         |   |
  WT   A      |   B         |   C
              |             |
         Candidate      Candidate
         optimum A      optimum B
```

| Candidate Optimum | Architecture | Hypothesis |
|-------------------|-------------|------------|
| A | Optimized CPU-mediated bounce (better pipelining, async overlap, buffer reuse) | Lower CPU via less wasted work, not path elimination |
| B | Direct / low-CPU path (NVMe DMA → GPU memory, CPU not in data path) | Lower CPU via architectural bypass |
| C | Hybrid tiered path (direct for large sequential, bounce for small/random) | Adaptive, workload-dependent |
| **Global optimum** | **Unknown — determined empirically by calibration and evolution** | |

We do NOT assume any candidate is the answer. The fitness function rewards observable
properties. Whatever architecture achieves high throughput + low CPU involvement wins.
Evolution may find A, B, C, or something we haven't considered.

The key question: **what data-movement architecture does LLM-guided evolution discover
when selected for GPU data-delivery throughput, tail latency, and correctness?**

Unlike random mutagenesis (which must cross valleys by neutral drift or multi-point
mutations), LLM proposers can reason about the code and potentially make coordinated
multi-site changes. This is analogous to rational design augmenting directed evolution —
the LLM acts as a reasoning layer that predicts which coordinated mutations might work.

**The positive control (Section 4.2) establishes a reference point**, not a guaranteed
global optimum. If a hand-written P2P path scores lower than optimized bounce, that is
itself a finding — it means the fitness landscape rewards practical performance over
architectural purity.

### 1.3 Pre-registration

Before running, we pre-register predictions:

| ID | Prediction | Falsified if |
|----|-----------|-------------|
| P1 | All frameworks will first optimize bounce (local climb) | Any framework's first accepted improvement changes the data path architecture |
| P2 | Search frameworks (AdaEvolve, EvoX) will plateau at ~0.60 | A search framework achieves >0.75 |
| P3 | Multi-file capable frameworks are more likely to discover path changes | A single-file framework achieves path change |
| P4 | The fitness valley (broken intermediates) will be observable | No framework produces a build-failing path-change attempt before succeeding |
| P5 | LLM reflection (seeing cpu_bypass feedback) is necessary for valley crossing | Random search discovers a path change |

### 1.4 Blinding

The evaluator is **implementation-blind**: it does not inspect function calls, check for
specific symbols, or reward named techniques. It measures only workload-relevant properties:
throughput, latency, scalability, and stability.

- Implementation-blind (no code inspection in scoring)
- Workload-first (throughput + latency + scalability = 85% of score)
- The scoring function has no concept of "P2P" — only observable performance

The path_verifier.py is used ONLY in post-hoc analysis (Section 5.3), never during scoring.

---

## 2. Fitness Function

### 2.1 Design Principles (from multi-objective optimization in materials science)

1. **Pareto-informative**: Each metric captures an independent physical property
2. **Hardware-grounded**: Ceilings and floors derived from physical constraints
3. **Monotonic**: Improving the real system always improves the score
4. **Discriminating**: The gap between local and global optima exceeds measurement noise
5. **Non-gameable**: No way to inflate score without improving real system behavior (see 2.9)

### 2.2 Metrics

| Metric | Physical meaning | Measurement method | Used in scoring? |
|--------|-----------------|-------------------|-----------------|
| `throughput_gbps` | Cold lookup data delivery rate | Benchmark output (aggregate GB/s) | **Yes (60%)** |
| `p99_latency_ms` | Worst-case per-object transfer time | Benchmark output (99th percentile) | **Yes (40%)** |
| `data_integrity` | Correctness of delivered data | Benchmark error count (ERRORS in output) | **Hard constraint** |
| `cpu_util_fraction` | CPU involvement | `/proc/stat` delta | Logged, not scored |
| `multi_client_throughput_gbps` | Aggregate throughput under contention | 8-client benchmark | Deferred (not yet implemented) |
| `throughput_cv` | Run-to-run stability | Coefficient of variation across repeats | Deferred (not yet implemented) |

**Current pilot scoring** (implemented in `evaluate_p2p.py`):
- 60% throughput + 40% latency
- Hard gate: data integrity, build success, parseable p99

**Deferred for final evaluation** (not awarding free points for unimplemented metrics):
- Multi-client scalability (requires second benchmark run per eval)
- Stability / CV (requires 3-5 repetitions per eval)

**Note on CPU**: Baseline is 3.2% on 64-core SPDK system — does not discriminate.
Logged for post-hoc classification only.

### 2.3 Scoring Formula

```python
THROUGHPUT_CEILING_GBPS = 12.0  # Above best observed (7.11 GB/s)
LATENCY_TARGET_MS = 0.4         # Below best observed p50 (382us)

def fitness(m: dict) -> float:
    if not m["build_succeeded"]:
        return 0.0
    if not m["data_integrity"]:
        return -1.0

    throughput = min(1.0, m["throughput_gbps"] / THROUGHPUT_CEILING_GBPS)
    latency    = min(1.0, LATENCY_TARGET_MS / max(0.01, m["p99_latency_ms"]))

    return 0.60 * throughput + 0.40 * latency
```

| Component | Weight | What it rewards | Ceiling source |
|-----------|--------|----------------|---------------|
| throughput | 60% | Higher cold-lookup GB/s | Set above best baseline (7.11 GB/s) |
| latency | 40% | Lower p99 cold-lookup latency | Set below best baseline p50 (382us) |

**Hard constraints** (not weighted — binary pass/fail):
- Build must succeed (score = 0.0)
- Data integrity must pass (score = -1.0)
- p99 latency must be parseable (score = 0.0)

**Why only throughput + latency?** On this SPDK-based system, CPU is already out of the
hot path (3.2% util on 64 cores). The performance ceiling is set by: NVMe read bandwidth,
pipeline ring depth, the cudaMemcpy H2D copy stage, and dispatch contention. An architecture
that eliminates a copy stage or deepens the pipeline will score higher on throughput + latency
without needing CPU-specific signals.

**Future additions** (when implemented):
- Multi-client scalability (run 8-client bench, reward aggregate throughput)
- Stability (run 3× and compute CV, penalize variance)

These are deferred to avoid awarding free points for metrics not yet measured.

### 2.4 Measured Baselines

Collected 2026-06-01. Tool: `certus-api-bench.py` (Python gRPC client; per-object latency
via `time.perf_counter()`; throughput = `block_size × objects × clients / wall_time`).
Block size: 4 MiB. Objects per batch: 16. Iterations: 10. Server restarted with `--format`
between drive configurations.

| Drives | Clients | Populate (agg GB/s) | Hot (agg GB/s) | Hot (per-client) | Cold (agg GB/s) | Cold (per-client) | Cold avg lat (us) |
|--------|---------|--------------------:|---------------:|-----------------:|----------------:|------------------:|------------------:|
| 1 | 1 | 10.05 | 15.63 | 15.83 | **2.39** | 2.39 | 1753 |
| 1 | 2 | 13.60 | 16.05 | 8.14 | 4.32 | 2.26 | 1857 |
| 1 | 4 | 8.34 | 19.29 | 4.97 | 6.00 | 1.69 | 2477 |
| 1 | 8 | 8.77 | 17.57 | 2.24 | 7.84 | 1.27 | 3304 |
| 2 | 1 | 10.53 | 15.75 | 16.00 | **6.63** | 6.68 | 628 |
| 2 | 2 | 13.34 | 20.51 | 10.39 | 8.73 | 4.65 | 903 |
| 2 | 4 | 13.69 | 19.29 | 5.03 | 9.77 | 2.87 | 1461 |
| 2 | 8 | 14.98 | 19.49 | 2.49 | 10.80 | 1.76 | 2377 |
| 4 | 1 | 10.76 | 16.08 | 16.30 | **6.94** | 6.98 | 601 |
| 4 | 4 | 15.44 | 19.46 | 6.34 | 10.10 | 3.05 | 1375 |
| 4 | 8 | 14.75 | 19.89 | 2.53 | 10.67 | 1.85 | 2271 |
| 7 | 1 | 10.24 | 16.19 | 16.41 | **7.11** | 7.16 | 586 |
| 7 | 2 | 13.79 | 17.05 | 8.62 | 8.47 | 4.66 | 901 |
| 7 | 4 | 14.44 | 20.17 | 5.18 | 9.78 | 2.97 | 1413 |
| 7 | 8 | 14.87 | 18.80 | 2.42 | **10.52** | 1.71 | 2450 |

CPU utilization (measured via `/proc/stat` delta during benchmark):
- 1 client: **3.2%** (64-core system)
- 8 clients: **3.55%**

Server startup: `certus-server --device-pci <addr> [--device-pci <addr>...] --format`
(uses `--format` to initialize extent managers fresh each run).

**VFIO note**: After a server crash or unclean shutdown, VFIO groups may remain held.
The evaluator kills the server in `finally` and waits 2s for VFIO release. If a drive
becomes unavailable (VFIO group busy), the evaluator will fail to start the server and
score 0 — the backup/restore still runs and the next eval retries cleanly.

### 2.5 Hardware Performance Ceilings (Measured)

All ceilings measured directly on the H8 machine, 2026-06-01.

| Resource | Measured | Tool | Notes |
|----------|---------|------|-------|
| Single NVMe sequential read | **5.41 GB/s** | `spdk_nvme_perf -q 32 -o 131072 -w read -t 10` | QD=32, 128K chunks, single drive (0000:62:00.0) |
| 7× NVMe aggregate | **37.3 GB/s** | `spdk_nvme_perf` (all 7 drives) | ~5.3 GB/s per drive, no contention between drives |
| cudaMemcpy H2D (pinned, 4 MiB) | **16.8 GB/s** | PyTorch `pin_memory=True` → `cuda:0`, `synchronize()` | Saturates at ~24 GB/s for 1 GiB transfers |
| cudaMemcpy H2D (pinned, 1 GiB) | **24.0 GB/s** | Same | PCIe Gen4 x16 practical ceiling |
| cudaMemcpy D2H | **17.0 GB/s** | Same (GPU → pinned host) | Asymmetric — D2H slower than H2D |
| Host DRAM bandwidth | **176 GB/s** | STREAM Triad, 64 threads, 100M array | Two-socket DDR4 system |
| GPU PCIe link | Gen4 x16 | `nvidia-smi --query-gpu=pcie.link.gen.current,pcie.link.width.current` | Theoretical 31.5 GB/s raw |
| Memory-tier → GPU (in system) | **16 GB/s** | `certus-api-bench.py` hot lookup | Matches H2D at 4 MiB size |

**Key takeaway for the experiment:**

The cold lookup path does: NVMe read (5.4 GB/s/drive) → host DRAM → cudaMemcpy H2D (16-24 GB/s).
With 2 drives, the NVMe aggregate (10.8 GB/s) exceeds the H2D bandwidth at 4 MiB size (16.8 GB/s),
so **cudaMemcpy H2D is NOT the bottleneck for ≤2 drives**. But with 7 drives (37.3 GB/s NVMe aggregate),
the H2D stage becomes the clear bottleneck — explaining why 7-drive cold (7.11 GB/s) is far below
the 37.3 GB/s NVMe ceiling.

A P2P path that eliminates the H2D copy would remove this bottleneck for multi-drive configurations,
letting throughput scale closer to the NVMe aggregate ceiling.

### 2.6 Baseline Analysis

**Key observations:**

1. **Single drive cold (2.39 GB/s) = 44% of measured drive ceiling (5.41 GB/s)** — pipeline
   overhead (ring management, GPU copy, gRPC dispatch) consumes ~56% of raw NVMe bandwidth.

2. **2 drives cold (6.63 GB/s) = 61% of 2× measured ceiling (10.8 GB/s)** — dispatcher
   shards across drives. Good scaling from 1→2 drives (2.8×).

3. **7 drives cold (7.11 GB/s) = only 19% of 7× measured ceiling (37.3 GB/s)** — a single
   client can't saturate 7 drives. The pipeline ring (8 buffers) and the cudaMemcpy H2D
   stage become the bottleneck. This is the main optimization opportunity.

4. **Hot lookup (~16 GB/s) is PCIe-limited** — memory-tier → GPU via cudaMemcpy H2D hits
   the PCIe x16 practical ceiling. Not improvable by path changes.

5. **Multi-client cold scales sub-linearly** — 1→8 clients on 2 drives: 6.63→10.80 GB/s
   (1.6× not 8×). Contention in the pipeline ring and dispatch Mutex.

6. **CPU is NOT the bottleneck** — 3.2% util on 64 cores. SPDK does userspace async DMA.
   CPU-based scoring would not discriminate between bounce and P2P on this system.

7. **The real bottlenecks are**: pipeline ring depth (limits parallelism), cudaMemcpy H2D
   stage (adds latency and consumes PCIe bandwidth that could go to NVMe), and dispatch
   contention under concurrency.

### 2.7 Reporting Metrics (Neutral, Applied to All Candidates)

Every candidate is reported on the same metric set:

```json
{
  "throughput_gbps": 0.0,
  "p99_latency_ms": 0.0,
  "p50_latency_ms": 0.0,
  "mean_latency_ms": 0.0,
  "multi_client_throughput_gbps": 0.0,
  "cpu_util_fraction": 0.0,
  "throughput_cv": 0.0,
  "data_integrity": true,
  "build_succeeded": true,
  "fitness_score": 0.0
}
```

### 2.8 Calibration Requirements (pre-experiment, mandatory)

Before evolution begins, verify ALL of:

| Condition | Expected | Action if violated |
|-----------|----------|-------------------|
| Wild-type fitness score | Compute from baselines above | Establishes the floor |
| Best positive control scores higher | Yes | If no control beats wild-type: scoring function is broken |
| Score gap (best control − wild-type) | ≥ 0.05 (pilot), ≥ 0.10 (main) | Adjust throughput/latency ceilings in fitness function |
| Score gap exceeds noise | gap > 3× CV | If not: pin CPU freq, increase benchmark iterations |
| Measurement noise (same code, 10 repeat evals) | CV < 0.05 | Pin CPU frequency, add warmup |
| Random mutations never exceed best control (30 trials) | True | If violated: experiment too easy |

### 2.9 Anti-Gaming Checks (mandatory per evaluation)

The fitness function can reward "doing less work" — a broken candidate that drops requests,
returns stale data, or transfers fewer bytes could appear low-CPU. Prevent this with
mandatory hard checks:

```python
# These are ALL hard failures (score = -1.0 if any fail)

if transferred_bytes != expected_bytes:
    return -1.0  # Incomplete transfer

if gpu_buffer_pattern != expected_pattern:
    return -1.0  # Data corruption

if request_count_completed != request_count_expected:
    return -1.0  # Dropped requests

if latency_suspiciously_low(p99_ms < 0.1):
    return -1.0  # Likely no-op or cached stale data
```

Logged per evaluation:
```json
{
  "expected_bytes": 67108864,
  "actual_bytes": 67108864,
  "gpu_pattern_verified": true,
  "request_count_pass": true,
  "stale_cache_check": "memory-tier cleared before cold lookup"
}
```

---

## 3. Mutagenesis Strategy (What Frameworks Receive)

### 3.1 Wild-Type (Seed Program)

The actual, unmodified source files:
- `components/dispatcher/src/pipeline.rs` — the transfer loop
- `components/dispatcher/src/lib.rs` — dispatcher component (buffer management, lifecycle)
- `components/gpu-services/src/dma.rs` — DMA buffer creation functions

No annotations, no EVOLVE-BLOCK markers, no hints about what to change.
The code is the code. Frameworks read it, understand it, mutate it.

### 3.2 Environmental Context (Given to All Frameworks)

```
## System Under Optimization

A storage server that moves data from NVMe SSDs to GPU memory for inference workloads.
Currently scored at ~0.55 on the fitness function. Higher scores are achievable.

## Hardware

- NVMe Gen4 SSDs via SPDK (userspace driver, no kernel filesystem)
- NVIDIA A30 GPU, PCIe Gen4 x16
- Kernel modules: nvidia-peermem (loaded), gdrdrv (loaded)
- 2048 hugepages, memlock unlimited, VFIO-bound NVMe devices

## Fitness Function

Measures: cold-lookup throughput and p99 tail latency.
Data correctness is a hard constraint (score = -1.0 on failure).

The primary scoring components reward fast and correct delivery of data to GPU
memory. CPU utilization and host-memory traffic are logged as diagnostic metrics
but are not used in scoring — baseline measurement showed 3.2% CPU on this 64-core
SPDK system, which does not discriminate between architectures.

## Build & Test

- cargo build -p certus-server --release
- The evaluator starts the server and runs a benchmark client automatically
- Your changes must compile and pass data integrity checks
- Data integrity is verified per-evaluation: byte count, pattern check, request count
```

**Note**: The context does not name "P2P", "GPU-direct", "bounce buffer", or any
implementation strategy. It lists hardware facts (modules loaded, devices present) and
the scoring signal (higher cold-lookup throughput + lower p99 latency). The frameworks
must discover any architectural opportunities from reading the codebase.

### 3.3 Mutation Scope

Frameworks may modify:
- `pipeline.rs` (transfer logic)
- `lib.rs` (buffer management, ring allocation, promote_and_serve)
- `dma.rs` (buffer creation — new allocation strategies)

Frameworks may NOT modify:
- Evaluator / fitness function
- Benchmark client
- gRPC service interface
- Build system
- Other components (interfaces, spdk-sys, etc.)

### 3.4 Framework Interface Strategies

**GEPA native** (multi-file dict):
- Seed: `{"pipeline.rs": ..., "lib.rs": ..., "dma.rs": ...}` (actual source files)
- Can mutate any/all files independently each iteration
- No concatenation needed

**Nous** (agentic, full repo access):
- Seed: the repository itself
- Can read any file, modify any in-scope file
- Natural multi-file capability
- Single-agent deep reasoning (design → execute → analyze loop)

**AutoScientists** (multi-agent, collaborative):
- Seed: the repository + TASK.md describing the optimization problem
- Multiple Claude Code subagents self-organize into teams
- Agents critique each other's proposals before spending eval compute
- Shared message board prevents redundant exploration
- Natural multi-file capability (each agent has full repo access)
- Longer-running: designed for hours/days of autonomous experimentation

**SkyDiscover frameworks** (AdaEvolve, EvoX, OpenEvolve):
- Seed: slim concatenated file (~300 lines) containing:
  - Full `pipeline.rs` (~200 lines) — the transfer loop
  - Key functions from `dma.rs` (~80 lines) — buffer creation APIs
  - `PipelineRing::new()` from `lib.rs` (~30 lines) — ring allocation
- Config: `diff_based_generation: false` (full rewrite mode, avoids diff-match failures)
- Config: `max_tokens: 16384` (ample for ~300 lines of full rewrite)
- Concatenation format (kept minimal):
  ```
  // --- FILE: pipeline.rs ---
  <full file>

  // --- FILE: dma.rs (buffer creation functions) ---
  <selected functions>

  // --- FILE: lib.rs (ring allocation) ---
  <PipelineRing::new and related>
  ```
- Evaluator splits on `// --- FILE: xxx ---`, patches each section back
- Only includes code the framework needs to see/change — not 1900 lines of lib.rs

**Why this works when prior H3 concatenation failed:**
1. Total size ~300 lines (not 1000+) — fits comfortably in full rewrite
2. `diff_based_generation: false` — no SEARCH/REPLACE matching to break
3. Simple separator format (no parenthetical line ranges or nested markers)
4. Each section is self-contained

### 3.5 Variant: Capability-Aware Context (H4-B)

In addition to the base experiment (H4-A: discovery without hints), run a variant
where frameworks receive a list of available primitives but NOT the final architecture:

```
## Available Memory/Data Movement Primitives

- Host pageable allocation (malloc)
- Host pinned allocation (cudaHostAlloc + spdk_mem_register)
- GPU device allocation (cudaMalloc)
- SPDK buffer registration (spdk_mem_register — makes any virtual address DMA-able)
- GPU memory registration with SPDK (if nvidia-peermem supports it)
- Async DMA copy APIs (cudaMemcpyAsync, dma_copy_to_device_async)
- CUDA streams for overlapped operations
```

This separates two questions:
1. Can the framework discover the architectural goal from fitness pressure alone? (H4-A)
2. Once given primitives, can it assemble the architecture? (H4-B)

Makes the negative result more informative — if H4-A fails but H4-B succeeds, the
bottleneck is discovery, not implementation.

### 3.6 Source Tree Safety — Mandatory Revert Guarantee

Every evaluation MUST leave the source tree in its original state regardless of
outcome (success, build failure, server crash, evaluator timeout, kill signal).

**Implementation**: The evaluator uses a `try/finally` pattern:

```python
# Restore stale backups from any previous crash (runs on import)
restore_stale_backups()

backups = {}
for target in ALL_TARGETS:
    bak = target.with_suffix(target.suffix + ".bak")
    shutil.copy2(target, bak)
    backups[target] = bak

# Signal handlers — restore on SIGTERM/SIGINT
signal.signal(signal.SIGTERM, restore_and_exit)
signal.signal(signal.SIGINT, restore_and_exit)

try:
    # patch, build, benchmark, score
    ...
finally:
    # UNCONDITIONAL restore — runs on success, failure, exception, signal
    for target, bak in backups.items():
        if bak.exists():
            shutil.copy2(bak, target)
            bak.unlink()
    kill_server()
    # Restore original signal handlers
```

**Additional safety measures:**
- On evaluator import: scan for stale `.bak` files from a previous crash and restore
- Signal handlers (SIGTERM, SIGINT): trigger restore before exit
- Evaluator never calls `git checkout` or `git reset` — uses file-level copy only
- Each evaluation is atomic: either the full cycle completes or everything reverts
- The `initial_programs/` directory holds pristine copies as a last-resort reference

---

## 4. Controls

### 4.1 Negative Control: Random Search

Random perturbations to numeric constants (ring size, timeout values, loop bounds).
Expected outcome: Score ~0.55-0.60 (mild throughput optimization, no path change).
Purpose: Establishes the "undirected mutagenesis" baseline — proves that path discovery
requires LLM reasoning, not luck.

### 4.2 Positive Controls: Manual Implementations (MANDATORY before main experiment)

Implement multiple known-different architectures by hand and score them all. This
maps the fitness landscape empirically rather than assuming its shape.

| Control | Implementation | Purpose |
|---------|---------------|---------|
| PC-1: Wild-type | Current code, no changes | Baseline (expected ~0.55) |
| PC-2: Optimized bounce | Larger ring, better pipelining, async overlap | Best bounce can do? |
| PC-3: GPU-direct (if feasible) | spdk_mem_register on GPU ptr, NVMe DMA to GPU | Test if P2P scores higher |
| PC-4: Hybrid | P2P for large, bounce for small | Adaptive path |

Steps:
1. Implement each control
2. Run all through the same evaluator
3. Record scores — **the highest-scoring control defines the landscape ceiling**
4. If PC-3 is infeasible (doesn't compile, crashes, or nvidia-peermem doesn't work
   with SPDK on this topology): that's a finding, proceed without it
5. If PC-2 (optimized bounce) scores as high as PC-3: the landscape is flat,
   meaning architectural change isn't rewarded — adjust weights or accept this finding

**Key insight**: We don't know which control scores highest until we measure. The
experiment's value doesn't depend on P2P being the winner — it depends on whether
evolution can find whatever the best architecture turns out to be.

### 4.3 Ablations

| Ablation | Scoring change | Purpose |
|----------|---------------|---------|
| A: Throughput-only | 100% throughput, 0% latency | Does latency pressure matter? |
| B: Latency-only | 0% throughput, 100% latency | Does evolution overfit small transfers? |
| C: Add CPU pressure | 40% throughput + 25% latency + 35% cpu_bypass | Does CPU signal change discovered architecture? |

Ablation C re-introduces the CPU scoring we removed. If C produces a different architecture
than the main run, that tells us the scoring signal shapes what evolution finds — even when
the main workload metric (throughput) would have rewarded it anyway.

### 4.4 Ablation: Disable P2P Modules (Perturbation Evidence)

For the best candidate that achieves path change, verify causally:
1. Unload nvidia-peermem (`sudo modprobe -r nvidia-peermem`)
2. Re-evaluate the candidate
3. Expected: performance collapses or candidate fails to start
4. If performance unchanged: the "path change" was illusory (false positive)

This provides Level 3 (perturbation) evidence that the path change is real,
not just a coincidental reduction in CPU usage from some other optimization.

---

## 5. Protocol

### 5.1 Pre-experiment (mandatory, abort if calibration fails)

1. Run preflight (`check_p2p_capability.sh`) — confirm hardware state
2. Build and run current code through evaluator 10× → record baseline score + noise (CV)
3. Implement positive control (manual path change) → record score
4. Verify score gap ≥ 0.05 between positive control and wild-type (≥ 0.10 for main run)
5. Run 30 random mutations → confirm none find path change
6. If any calibration step fails: diagnose and fix before proceeding

### 5.2 Main Experiment

Budget structure (pilot → scale if results justify):

| Phase | Budget per framework | Purpose |
|-------|---------------------|---------|
| Pilot | 30 evals | Validate setup, check for obvious failures |
| Small | 100 evals | Sufficient for local optimization plateau |
| Main | 300 evals | Allow valley crossing attempts |

For each framework in {GEPA native, AdaEvolve, EvoX, OpenEvolve, Nous, AutoScientists, Random}:
1. Fresh seed (unmodified wild-type code)
2. Same fitness function
3. Same background context (H4-A variant; repeat with H4-B for comparison)
4. Record: all scores, all candidates, wall time, LLM token cost, build/eval call count

Nous budget: 3 deep iterations (normalize comparison by token cost and eval count,
not raw iteration count).

### 5.3 Post-experiment Analysis

For each framework's best candidate:
1. Classify approach: {knob_tuning, pipeline_restructure, path_change, hybrid}
2. Three-level path verification:
   - **Level 1 (static)**: Binary symbol analysis, patch diff inspection
   - **Level 2 (runtime)**: GPU PCIe RX traffic, CPU memory bandwidth, cudaMemcpy absence
   - **Level 3 (perturbation)**: Disable nvidia-peermem → performance collapses? (Section 4.4)
3. Plot fitness trajectory (score vs iteration count, score vs dollar cost)
4. Identify "valley crossing" events (score drop followed by score jump)
5. Compare H4-A vs H4-B (does capability-aware context help?)
6. Normalize across frameworks: score/dollar, score/eval, time-to-best

---

## 6. Expected Outcomes and Significance

### If evolution discovers a novel architecture (path change or otherwise):
- Demonstrates LLM-guided evolution can make architectural leaps (not just parameter tuning)
- Identifies which framework properties enable it (multi-file? reflection? agency?)
- Establishes physical-property scoring as effective selection pressure for architecture search
- The specific architecture discovered is itself a finding (may not be P2P)

### If evolution optimizes within the existing architecture only:
- Quantifies the difficulty of architectural transformation via evolution
- Identifies where frameworks get stuck (plateau analysis, failed attempts)
- Measures the local-optimum ceiling (how good can optimized bounce get?)
- Informs what additional affordances (hints, primitives, examples) are needed
- Still valuable: characterizes the boundary between "optimization" and "invention"

### If optimized bounce scores as well as or better than P2P:
- The fitness landscape is flatter than hypothesized
- Architectural change is not necessary for this workload — a valid finding
- Suggests the current bottleneck (pipeline ring depth, dispatch contention) can be
  solved within the existing architecture without eliminating the H2D copy

### Either way:
- Establishes a methodology for scoring data-path architecture (not just throughput)
- Maps the fitness landscape empirically via positive controls
- Demonstrates anti-gaming verification for evolutionary code optimization
- Produces a calibrated fitness function for future evolution work

### Contribution framing:

> Can LLM-guided evolution discover systems-level architecture changes under
> physical selection pressure — and if not, what is the boundary between
> parameter optimization and architectural invention?

The contribution is the **methodology and the characterization** — not a specific
performance number or a specific architecture.

---

## 7. Results Structure

Each framework gets its own results directory with full traceability: every candidate,
every score, and a post-run analysis.

```
results/
├── gepa_native/
│   ├── scores.jsonl              # One line per eval: {iteration, score, metrics}
│   ├── candidates/              # Every candidate tested (numbered)
│   │   ├── 001_pipeline.rs
│   │   ├── 001_lib.rs
│   │   ├── 001_dma.rs
│   │   ├── 002_pipeline.rs
│   │   └── ...
│   ├── best/                    # Best-scoring candidate files
│   │   ├── pipeline.rs
│   │   ├── lib.rs
│   │   └── dma.rs
│   └── analysis.md             # Post-run analysis (auto-generated)
├── adaevolve/
│   └── ...
├── evox/
│   └── ...
├── openevolve/
│   └── ...
├── nous/
│   └── ...
├── autoscientists/
│   └── ...
├── random/
│   └── ...
└── summary/
    ├── pareto_frontier.json         # All best candidates, all metrics
    ├── trajectory_plots/            # Score vs iteration per framework
    └── final_analysis.md            # Cross-framework comparison
```

### 7.1 Per-Run Analysis (generated after each framework completes)

Each `analysis.md` contains:

```markdown
# Analysis: {framework}

## Summary
- Best score: X.XXXX (iteration N)
- Wild-type baseline: X.XXXX
- Improvement over baseline: +X.XX%
- Total evaluations: N (M build failures, K integrity failures)
- Wall time: Xm Xs
- LLM cost: $X.XX

## What Changed (diff analysis of best candidate vs wild-type)
- Files modified: [pipeline.rs, lib.rs, ...]
- Lines changed: +N / -M
- Key structural changes:
  - [e.g., "Added gpu buffer allocation in PipelineRing::new()"]
  - [e.g., "Increased PIPELINE_RING_SIZE from 8 to 32"]
  - [e.g., "Replaced cudaMemcpy H2D with direct NVMe → GPU DMA"]

## Architecture Classification
- Category: {knob_tuning | pipeline_restructure | path_change | hybrid | other}
- Path evidence:
  - Level 1 (static): {symbols found in binary, diff shows new DMA calls}
  - Level 2 (runtime): {throughput jumped from X to Y, latency dropped}
  - Level 3 (perturbation): {pending — run Section 4.4 if path_change detected}

## Fitness Trajectory
- Iteration 1: 0.XX (first accepted)
- Iteration N: 0.XX (plateau start)
- Iteration M: 0.XX (breakthrough / final best)
- Valley crossings detected: {count, iterations where score dropped then recovered}

## Failed Attempts (interesting failures)
- Iteration X: attempted [description], build failed because [reason]
- Iteration Y: compiled but integrity check failed because [reason]
```

### 7.2 Final Cross-Framework Analysis

After all runs complete, `summary/final_analysis.md` compares:

1. **Score table**: best score per framework
2. **Architecture discovered**: what each framework found (knob tuning vs path change)
3. **Cost efficiency**: score-per-dollar, score-per-eval across frameworks
4. **Pareto frontier**: throughput vs latency with all best candidates plotted
5. **Valley crossing**: which frameworks attempted and failed path changes before succeeding?
6. **H4-A vs H4-B comparison**: did capability-aware context help?

---

## 8. File Inventory

| File | Purpose |
|------|---------|
| `evolve_p2p_experiment.md` | This document |
| `preflight/check_p2p_capability.sh` | Hardware capability classifier |
| `baselines/run_baselines.sh` | Pre-experiment baseline collection |
| `evaluator/evaluate_p2p.py` | Fitness function implementation (workload-first, blind) |
| `evaluator/path_verifier.py` | Post-hoc path classification (NOT used in scoring) |
| `evaluator/analyze_run.py` | Post-run analysis generator (creates analysis.md) |
| `initial_programs/` | Wild-type seed files (copies of current source) |
| `run_gepa_p2p.py` | GEPA native runner |
| `configs/` | Per-framework configuration |
| `results/` | Per-framework × profile results (see Section 7) |
| `controls/positive/` | Hand-written implementations for calibration |
| `controls/negative/` | Random search results |
| `controls/ablation/` | Throughput-only scoring results |
| `controls/perturbation/` | nvidia-peermem disable test results |

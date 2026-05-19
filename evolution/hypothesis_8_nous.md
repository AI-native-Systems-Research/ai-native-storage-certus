# Hypothesis 8 — Nous Experiment Analysis

## Objective

Evaluate Nous's autonomous experiment capability on a GPU storage transfer optimization hypothesis, and determine whether pipelined bounce-buffer (SSD→CPU→GPU) transfers outperform direct SSD→GPU P2P DMA for 4 MiB objects at 128 KiB NVMe chunk size.

> **Hypothesis:** Using a bounce buffer SSD→CPU→GPU with pipelined transfers is faster than direct SSD→GPU for transfer of 4 MiB broken into a stream of 128 KiB transfers.

---

## Experiment Overview

**Hypothesis given to Nous:** Pipelined bounce-buffer SSD→CPU→GPU is faster than direct SSD→GPU P2P for 4 MiB at 128 KiB chunks.

**What Nous should have done:** Implement pipelining (concurrent NVMe reads + GPU copies) in the bounce path, then compare against P2P — through the actual dispatcher (`certus-server`).

| Run | What Nous decided to do | System used | Key Result | Cost | Status |
|-----|------------------------|-------------|------------|------|--------|
| h8-transfer-path | Compare existing (non-pipelined) bounce vs P2P | `gpu-p2p-server` (test binary) | P2P 2x faster | $9.57 | DONE |
| h8-pipelined | Implement pipelining in bounce, compare vs P2P | `gpu-p2p-server` (test binary) | 17% gain (buggy impl) | $5.28 | DONE |
| h8-evolve-v0 | Fix pipelining bug (channel reuse) | `gpu-p2p-server` (test binary) | Stuck at iter-2 | $7.93 | STUCK |
| h8-dispatcher-p2p | Decompose path effect vs submission strategy | `gpu-p2p-server` (test binary) | P2P-seq 1.47x faster | $10.41 | DONE |
| h8-v0-vs-p2p | Compare bounce vs P2P (constrained to dispatcher) | `certus-server` (full system) | P2P 1.33x **slower** | $7.66 | PARTIAL |
| h8-v1-vs-p2p | Compare pipelined bounce vs P2P (constrained) | `certus-server` (full system) | No data (budget exhausted) | ~$7.32 | FAILED |
| **Total** | | | | **~$48.17** | |

All runs used Opus for design, Sonnet for execute_analyze.

The first 4 runs (h8-transfer-path, h8-pipelined, h8-evolve-v0, h8-dispatcher-p2p) all used `gpu-p2p-server` — a standalone test binary that exists in the repo for validating P2P DMA in isolation. It talks directly to NVMe + GPU, bypassing the entire dispatcher stack (gRPC, extent-manager, memory-tier, dispatch-map). Nous found this binary on its own while exploring the codebase and decided to use it instead of certus-server because it's simpler to instrument and doesn't require understanding the full system.

Only after adding explicit constraints to the campaign description ("Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server.") did the last 2 runs (h8-v0-vs-p2p, h8-v1-vs-p2p) test through the actual system. This revealed that isolated results were misleading — P2P goes from 1.47x faster in the test binary to 1.33x slower through the dispatcher.

---

## Methodology

### Nous Bundle Structure

Each experiment is a bundle with up to 4 arms:

| Arm | Purpose |
|-----|---------|
| h-main | Core A-vs-B comparison |
| h-control-negative | Known-bad baseline (validates measurement) |
| h-ablation | Remove one mechanism to isolate causation |
| h-robustness | Vary parameters to test generalization |

### What Nous Was Given

Minimal campaign: research question, the full repository (all source code), target binary path (`certus-server`), observable metrics (throughput, latency), controllable knobs (transfer mode, chunk size). No prior findings, no handoff, no implementation hints. The intent was always to test through the full dispatcher stack.

---

## Results

### 1. Isolated DMA Path Characterization

**h8-transfer-path** (2 iterations, harness):

Iter-1 tested existing bounce vs P2P with no code changes.

| Mode | Throughput | Avg Latency |
|------|-----------|-------------|
| Bounce (sequential) | 1510 MB/s | 2.65 ms |
| P2P warm (pre-pinned) | 3031 MB/s | 1.32 ms |
| P2P cold (per-request pin) | 535 MB/s | 7.49 ms |

Iter-2 decomposed latency into NVMe read phase vs GPU copy phase:

| Mode | NVMe Read | Copy | Total |
|------|-----------|------|-------|
| Bounce | 790 μs | 819 μs (H2D) | ~1610 μs |
| P2P warm | 710 μs | 114 μs (D2D) | ~824 μs |

The copy phase is 7x slower in bounce (H2D at ~4.9 GB/s vs D2D at ~35 GB/s). NVMe read time is nearly identical regardless of DMA target. The 2x end-to-end gap exists because NVMe read time (~750μs) dilutes the 7x copy difference.

**h8-dispatcher-p2p** (2 iterations, harness):

Isolated the P2P path advantage from the submission strategy (sequential ReadSync vs BatchSubmit):

| Condition | Avg Latency | Throughput |
|-----------|-------------|-----------|
| P2P sequential | 1.58 ms | 2534 MB/s |
| Bounce sequential | 2.32 ms | 1724 MB/s |
| P2P batch | 1.10 ms | 3641 MB/s |
| Bounce batch | 2.73 ms | 1465 MB/s |

P2P-seq is 1.47x faster than bounce-seq. Surprise: bounce-batch avg (2.73ms) is WORSE than bounce-seq (2.32ms) due to NVMe queue saturation causing 10-11ms tail spikes. P2P-batch has no such problem (max-min spread <0.03ms).

### 2. Pipelining Implementation

**h8-pipelined** (1 iteration, test binary):

Nous implemented true pipelining (concurrent NVMe reads + cudaMemcpyAsync) in `gpu-p2p-server`:

| Condition | Throughput | Avg Latency |
|-----------|-----------|-------------|
| Sequential bounce (baseline) | 1440 MB/s | 2.78 ms |
| Pipelined bounce (new) | 1764 MB/s | 2.27 ms |
| P2P warm (reference) | 3082 MB/s | 1.30 ms |

Only 17% gain instead of predicted ~50%. Root cause: `connect_client()` called per chunk (32×17μs = 544μs overhead) became the new bottleneck. However, the async cudaMemcpy overlap IS working — copy dispatch drops from 826μs synchronous → 112μs async, confirming SPDK hugepages satisfy CUDA's pinned-memory requirement.

**h8-evolve-v0** (1.5 iterations, test binary):

Follow-up to fix the `connect_client()` bug by reusing a single channel across all chunks. Iter-1 reproduced the same results as h8-pipelined. Iter-2 (channel reuse fix) was designed but never executed — campaign stuck at EXECUTE_ANALYZE.

### 3. End-to-End Dispatcher Validation

**h8-v0-vs-p2p** (dispatcher v0, certus-server):

First run through the actual system (gRPC → dispatcher → NVMe → GPU). Campaign constraints forced Nous to use certus-server instead of the harness.

| Condition | SSD Avg Latency | Throughput |
|-----------|----------------|-----------|
| Bounce (4 MiB) | 13,764 μs | 0.30 GB/s |
| P2P (4 MiB) | 18,372 μs | 0.23 GB/s |
| Bounce (1 MiB) | 5,228 μs | 0.20 GB/s |
| P2P (1 MiB) | 5,665 μs | 0.19 GB/s |

**P2P is 1.33x SLOWER than bounce through the dispatcher.** Root cause: cold pinning per request — the P2P implementation calls `prepare_memory_for_spdk()` on every lookup instead of maintaining a persistent staging pool. Per prior results, cold P2P is 2.74x slower than bounce.

Also notable: dispatcher bounce (13.7ms) is 6x slower than harness bounce (2.3ms). The overhead comes from gRPC serialization, dispatch-map lookup, extent-manager resolution, per-segment DMA buffer allocation, and memory-tier management.

**h8-v1-vs-p2p** (dispatcher v1, certus-server):

Design phase completed: correct problem framing, validated v1 baseline (SSD-tier avg 3567μs for 4 MiB), identified all code change targets. Executor spent 120 turns implementing P2P in v1's complex pipeline and never reached benchmarking. Re-running with 200-turn limit.

---

## Key Findings

1. **Harness results don't predict system behavior** — P2P is 1.47x faster in isolation but 1.33x slower through the dispatcher due to integration overhead (cold pinning)
2. **Pre-pinned staging is mandatory** — cold P2P (per-request pin/unpin) is 2.74x slower than bounce; every harness run confirmed this but the first dispatcher implementation still got it wrong
3. **System overhead dominates** — dispatcher SSD lookup is 13.7ms vs harness 2.3ms; the DMA path optimization (saving ~0.7ms) is only 5% of total latency
4. **Pipelining is viable** — async cudaMemcpy overlap confirmed working; SPDK hugepages satisfy CUDA pinned-memory requirement; 17% gain limited by per-chunk channel allocation bug
5. **Sequential submission is safer for bounce** — BatchSubmit causes tail amplification on bounce path; P2P is immune
6. **The hypothesis remains untested end-to-end** — no run has tested pipelined bounce (with pre-pinned staging pool) against P2P through the actual dispatcher

---

## Nous Capability Assessment

**Strengths:**
- Code discovery: found all transfer modes, identified GDRCopy overhead, noted MDTS constraint, correctly mapped dispatcher architecture
- Experiment design: clean controls, reproducible conditions, correct measurement protocols
- Instrumentation: high-quality latency decomposition in iter-2 (per-phase breakdown is exactly what we needed)

**Critical failure — no hypothesis-to-experiment alignment:**

Nous never tested the hypothesis. The research question says "with pipelined transfers" but the existing code does sequential two-phase (read-all-then-copy-all). Nous identified this gap in iter-1 design notes:

> "Reading the code revealed there is NO pipelining — both modes do read-all-then-copy-all sequentially."

But conditioned implementing pipelining on "if bounce wins" — circular logic since bounce can't win without pipelining. When bounce lost, it pivoted to diagnostics. Iter-2's own data shows NVMe read (790μs) ≈ H2D copy (819μs) — the ideal scenario for pipelining — and Nous measured this evidence without recognizing it.

**Failure modes observed:**
1. No hypothesis-to-experiment alignment check (tested a different question than asked)
2. Path of least resistance (used standalone harness until explicitly constrained to use the full system)
3. Implementation bugs become blockers (`connect_client()` per chunk, cold pinning)
4. Budget exhaustion on complex implementations (v1 P2P: 120 turns, no data)

**Recommendations for Nous development:**
1. Add `constraints` field to campaign schema (hard rules validated before execution)
2. Weight keywords in hypothesis — flag if experiment doesn't address them
3. Hypothesis-to-experiment alignment gate (reject bundle if it doesn't test what's stated)

---

## Open Questions

1. **Where is the 11ms non-DMA overhead?** Dispatcher bounce is 13.7ms vs harness 2.3ms — profiling needed to identify the bottleneck (gRPC? extent-manager? per-segment allocation?)
2. **Will pre-pinned P2P win through the dispatcher?** Current P2P implementation does cold pinning; a persistent staging pool should recover the 1.47x advantage seen in harness
3. **Can pipelined bounce match P2P?** Iter-2 data predicts ~820μs with true pipelining vs P2P's 824μs — nearly identical. But system overhead may dominate either way
4. **Is the optimization worth pursuing?** If DMA path is only 5% of total dispatcher latency, the system overhead is the real target

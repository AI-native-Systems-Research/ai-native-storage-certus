# Hypothesis 8 — Nous Experiment Analysis

## Objective

Evaluate Nous's autonomous experiment capability on a GPU storage transfer optimization hypothesis, and determine whether pipelined bounce-buffer (SSD→CPU→GPU) transfers outperform direct SSD→GPU P2P DMA for 4 MiB objects at 128 KiB NVMe chunk size.

```
Path A: Pipelined Bounce (hypothesis predicts this wins)

  ┌─────┐     NVMe DMA      ┌──────────┐   cudaMemcpyAsync   ┌─────┐
  │ SSD │ ─────────────────>│ Host RAM │ ──────────────────> │ GPU │
  └─────┘   (128 KiB chunks)└──────────┘   (overlapped)      └─────┘
                 ↕ pipelined: next chunk reads while previous copies

Path B: Direct P2P DMA

  ┌─────┐     NVMe DMA to GPU BAR1      ┌─────┐
  │ SSD │ ─────────────────────────────>│ GPU │
  └─────┘   (128 KiB chunks, no host)   └─────┘
                 ↕ no intermediate copy, but limited by BAR1 bandwidth
```

---

## Experiment Overview

**Base hypothesis (given to all runs):**
> Using a bounce buffer SSD→CPU→GPU with pipelined transfers is faster than direct SSD→GPU for transfer of 4 MiB broken into a stream of 128 KiB transfers.

**What exists in the codebase:**
- **Dispatcher:** The component inside `certus-server` responsible for SSD→GPU data movement. Handles NVMe reads, memory-tier promotion, and GPU copies. `certus-server` is the full system (gRPC + dispatcher + extent-manager + memory-tier). The hypothesis should be tested through `certus-server` to exercise the dispatcher in context.
  - **v0:** Sequential bounce — reads all 128 KiB chunks to host DRAM via ReadSync, then single `dma_copy_to_device` to GPU. No pipelining, no P2P.
  - **v1:** "Pipelined" bounce — ring of 4 DMA buffers, per-chunk ReadSync + GPU copy. Despite the name, it's sequential per-chunk (no overlap between read and copy stages). No P2P.
- **`gpu-p2p-server`:** Standalone test binary for validating P2P DMA in isolation. Talks directly to NVMe + GPU, bypasses entire dispatcher stack. Has bounce/P2P/P2P-cold modes but no pipelining.
- **Neither dispatcher has P2P or true pipelining.** We want to see whether Nous can discover this gap and implement what's missing to properly test the hypothesis.

| Run | Additional constraints | What Nous implemented | Key Result | Cost | Status |
|-----|----------------------|----------------------|------------|------|--------|
| h8-transfer-path | *(none — base hypothesis only)* | Nothing — found `gpu-p2p-server` (standalone test binary) and used it to compare existing modes, not the actual data path | **Hypothesis not tested** (no pipelining exists, wrong binary); P2P 2x faster | $9.57 | DONE |
| h8-pipelined | + "Must use pipelined implementation, implement if not present" | Implemented pipelining in `gpu-p2p-server` (not in dispatcher): overlapping NVMe reads with async GPU copies | **Hypothesis not tested on actual system**; pipelining shows 17% gain but in wrong binary, buggy impl (`connect_client` per chunk) | $5.28 | DONE |
| h8-dispatcher-p2p | *(base hypothesis, campaign description pointed to dispatcher v1)* | Added sequential ReadSync variants to isolate path vs submission strategy | **Hypothesis not tested on actual system**; P2P-seq 1.47x faster in test binary | $10.41 | DONE |
| h8-v0-vs-p2p | + **"Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server"** --dispatcher-version v0 | P2P read path in dispatcher v0 (per-request GPU memory pinning) | **Correctly tested on actual system**; P2P 1.33x slower — cold pinning kills advantage | $7.66 | PARTIAL |
| h8-v1-vs-p2p | + **"Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server"** --dispatcher-version v1 | P2P read path in dispatcher v1 (per-request GPU memory pinning) | **Correctly tested on actual system**; P2P 1.18x slower — same cold pinning issue as v0. First attempt (120 turns, $7.32) failed with no data; succeeded at 200 turns ($9.14). | ~$16.46 | DONE |
| **Total** | | | | **~$49.38** | |

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

### 1. h8-transfer-path (2 iterations)

Tested existing (non-pipelined) bounce vs P2P with no code changes.

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

### 2. h8-pipelined (1 iteration)

Implemented true pipelining (concurrent NVMe reads + cudaMemcpyAsync) in `gpu-p2p-server`:

| Condition | Throughput | Avg Latency |
|-----------|-----------|-------------|
| Sequential bounce (baseline) | 1440 MB/s | 2.78 ms |
| Pipelined bounce (new) | 1764 MB/s | 2.27 ms |
| P2P warm (reference) | 3082 MB/s | 1.30 ms |

Only 17% gain instead of predicted ~50%. Root cause: `connect_client()` called per chunk (32×17μs = 544μs overhead) became the new bottleneck. However, the async cudaMemcpy overlap IS working — copy dispatch drops from 826μs synchronous → 112μs async, confirming SPDK hugepages satisfy CUDA's pinned-memory requirement.

### 3. h8-dispatcher-p2p (2 iterations)

Isolated the P2P path advantage from the submission strategy (sequential ReadSync vs BatchSubmit):

| Condition | Avg Latency | Throughput |
|-----------|-------------|-----------|
| P2P sequential | 1.58 ms | 2534 MB/s |
| Bounce sequential | 2.32 ms | 1724 MB/s |
| P2P batch | 1.10 ms | 3641 MB/s |
| Bounce batch | 2.73 ms | 1465 MB/s |

P2P-seq is 1.47x faster than bounce-seq. Surprise: bounce-batch avg (2.73ms) is WORSE than bounce-seq (2.32ms) due to NVMe queue saturation causing 10-11ms tail spikes. P2P-batch has no such problem (max-min spread <0.03ms).

### 4. h8-v0-vs-p2p (1 iteration)

First run through the actual system (gRPC → dispatcher → NVMe → GPU). Campaign constraints forced Nous to use `certus-server`.

| Condition | SSD Avg Latency | Throughput |
|-----------|----------------|-----------|
| Bounce (4 MiB) | 13,764 μs | 0.30 GB/s |
| P2P (4 MiB) | 18,372 μs | 0.23 GB/s |
| Bounce (1 MiB) | 5,228 μs | 0.20 GB/s |
| P2P (1 MiB) | 5,665 μs | 0.19 GB/s |

**P2P is 1.33x SLOWER than bounce through the dispatcher.** Root cause: cold pinning per request — the P2P implementation calls `prepare_memory_for_spdk()` on every lookup instead of maintaining a persistent staging pool. Per prior results, cold P2P is 2.74x slower than bounce.

Also notable: dispatcher bounce (13.7ms) is 6x slower than test binary bounce (2.3ms). The overhead comes from gRPC serialization, dispatch-map lookup, extent-manager resolution, per-segment DMA buffer allocation, and memory-tier management.

### 5. h8-v1-vs-p2p (1 iteration)

Tested P2P vs pipelined bounce through `certus-server --dispatcher-version v1`. Required 200 turns ($9.14 executor cost alone; first attempt at 120 turns failed without data).

| Condition | SSD Avg Latency | SSD Min Latency | Throughput |
|-----------|----------------|----------------|-----------|
| Bounce v1 (4 MiB) | 12,969 μs | 11,424 μs | 0.32 GB/s |
| P2P (4 MiB) | 15,239 μs | 13,919 μs | 0.28 GB/s |
| Bounce v1 (4 KiB) | 460 μs | 244 μs | 0.01 GB/s |
| P2P (4 KiB) | 496 μs | 233 μs | 0.01 GB/s |

**P2P is 1.18x slower than bounce v1** — same direction as v0 (1.33x), slightly less severe. Same root cause: cold pinning per request via `prepare_memory_for_spdk()`. At 4 KiB (control-negative), difference is negligible (~8%), confirming the mechanism is in the bulk transfer path.

Notable: v1 bounce (12.97ms) is slightly faster than v0 bounce (13.76ms) for the same 4 MiB — the ring-buffer per-chunk approach has marginal benefit over v0's read-all-then-copy.

---

## Key Findings

1. **Harness results don't predict system behavior** — P2P is 1.47x faster in isolation but 1.33x slower through the dispatcher due to integration overhead (cold pinning)
2. **Pre-pinned staging is mandatory** — cold P2P (per-request pin/unpin) is 2.74x slower than bounce; every harness run confirmed this but the first dispatcher implementation still got it wrong
3. **System overhead dominates** — dispatcher SSD lookup is 13.7ms vs harness 2.3ms; the DMA path optimization (saving ~0.7ms) is only 5% of total latency
4. **Pipelining is viable** — async cudaMemcpy overlap confirmed working; SPDK hugepages satisfy CUDA pinned-memory requirement; 17% gain limited by per-chunk channel allocation bug
5. **Sequential submission is safer for bounce** — BatchSubmit causes tail amplification on bounce path; P2P is immune
6. **The hypothesis remains untested properly** — both dispatcher runs show P2P slower, but because Nous implemented per-request GPU memory pinning (`prepare_memory_for_spdk()` on every lookup) instead of a persistent pre-pinned staging pool. Cold pin/unpin adds ~5-9ms per request, negating the P2P advantage. No run has tested pipelined bounce against P2P with pre-pinned staging.

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

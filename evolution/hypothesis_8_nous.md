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
- **Test clients (pre-existing):**
  - `apps/certus-server/python-client/test_client.py` — gRPC benchmark client for `certus-server`. Populates objects, forces eviction to SSD, then measures per-object lookup latency (`time.perf_counter()` around `stub.Lookup`) and throughput (`object_size / latency`). Reports avg/min/max latency (μs) and GB/s for both memory-tier and SSD-tier lookups.
  - `components/gpu-services/v0/tests/gpu_client_p2p.py` — Unix socket client for `gpu-p2p-server`. Sends transfer requests, measures per-transfer latency and throughput (MB/s). Reports avg/min/max latency (ms).
- **Neither dispatcher has P2P or true pipelining.** We want to see whether Nous can discover this gap and implement what's missing to properly test the hypothesis.

| Run | Additional constraints | What Nous implemented | Key Result | Cost |
|-----|----------------------|----------------------|------------|------|
| h8-transfer-path | *(none — base hypothesis only)* | Nothing — found `gpu-p2p-server` (standalone test binary) and used it to compare existing modes, not the actual system | **Hypothesis not tested** (no pipelining exists, wrong binary); P2P 2x faster | $9.57 |
| h8-pipelined | + "Must use pipelined implementation, implement if not present" | Implemented pipelining in `gpu-p2p-server` (not in dispatcher): overlapping NVMe reads with async GPU copies | **Hypothesis not tested on actual system**; iter-1: 17% gain (buggy `connect_client` per chunk); iter-2: 2.4-3x faster than sequential bounce with cudaHostAlloc fix, but still slower than P2P warm | $15.49 |
| h8-dispatcher-p2p | *(base hypothesis, campaign description explicitly pointed to dispatcher v1)* | Added sequential ReadSync variants to isolate path vs submission strategy | **Hypothesis not tested on actual system**; P2P-seq 1.47x faster in test binary | $10.41 |
| h8-v0-vs-p2p | + **"Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server"** --dispatcher-version v0 | P2P read path in dispatcher v0 (per-request GPU memory pinning) | **Correctly tested on actual system**; P2P 1.33x slower — cold pinning kills advantage. Hit budget limit before writing findings.json. | $7.66 |
| h8-v1-vs-p2p | + **"Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server"** --dispatcher-version v1 | P2P read path in dispatcher v1 (per-request GPU memory pinning) | **Correctly tested on actual system**; P2P 1.18x slower — same cold pinning issue as v0. First attempt (120 turns, $7.32) failed with no data; succeeded at 200 turns ($9.14). | ~$16.46 |
| h8-v0-pinned | + "even with pre-pinned GPU memory" --dispatcher-version v0 | P2P with persistent staging in dispatcher v0 (changed IDispatcher interface, 12 files, 597-line patch) | **Budget exhausted** (240 turns); implementation too complex — interface change cascaded to all dispatchers, benchmarks, tests. No benchmark data produced. | $14.10 |
| h8-v1-pinned | + "even with pre-pinned GPU memory" --dispatcher-version v1 | *(queued)* | *(pending)* | — |
| **Total** | | | | **~$73.69** |

All runs used Opus for design, Sonnet for execute_analyze.

The first 4 runs (h8-transfer-path, h8-pipelined, h8-evolve-v0, h8-dispatcher-p2p) all used `gpu-p2p-server` — a standalone test binary that exists in the repo for validating P2P DMA in isolation. It talks directly to NVMe + GPU, bypassing the entire dispatcher stack (gRPC, extent-manager, memory-tier, dispatch-map). Nous found this binary on its own while exploring the codebase and decided to use it instead of certus-server because it's simpler to instrument and doesn't require understanding the full system.

Only after adding explicit constraints to the campaign description ("Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server.") did the last 2 runs (h8-v0-vs-p2p, h8-v1-vs-p2p) test through the actual system. This revealed that isolated results were misleading — P2P goes from 1.47x faster in the test binary to 1.33x slower through the dispatcher.

The pinned campaigns (h8-v0-pinned, h8-v1-pinned) add only "even with pre-pinned GPU memory" to the research question — no implementation hints about persistent pools or amortized pinning. The Opus designer autonomously discovered prior experiment data in `.nous/h8-v0-vs-p2p/` and `.nous/h8-v1-vs-p2p/`, read their findings (1.33x slower due to cold pinning overhead), and designed the new experiment specifically to amortize the pin cost: one-time GPU buffer preparation at init, reused across all lookups. This demonstrates that Nous can compound knowledge across campaigns when prior results are accessible in the repository.

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

| Arm | Prediction |
|-----|-----------|
| h-main | Bounce will achieve lower latency than P2P warm (H2D copy cheaper than GDRCopy overhead) |
| h-control-negative | P2P cold will be slower than P2P warm (per-request GDRCopy pin/unpin overhead) |
| *iter-2 adds:* | |
| h-main | Copy phase in bounce will account for >60% of total latency; bounce copy >1.5x longer than P2P |
| h-ablation | Copy-only (--skip-nvme): bounce H2D will be >1.5x longer than P2P D2D |
| h-control-negative | NVMe read phase will be equal between bounce and P2P (within 20%) |

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

**Nous findings:** Iter-1 predicted bounce would win (wrong) — diagnosed that 32 sequential H2D copies of 128 KiB are the bottleneck, not NVMe DMA target selection. P2P warm eliminates H2D entirely. Cold P2P overhead (~187μs per chunk for GDRCopy setup) was larger than expected. Iter-2 confirmed copy ratio of 7.2x (much larger than predicted >1.5x), and proved NVMe read phase is equal regardless of DMA target — the difference is entirely in the copy stage.

### 2. h8-pipelined (2 iterations)

Implemented true pipelining (concurrent NVMe reads + cudaMemcpyAsync) in `gpu-p2p-server`.

| Arm | Prediction |
|-----|-----------|
| h-main (iter-1) | Pipelined bounce will reduce latency by 40-50% vs non-pipelined, approaching P2P warm |
| h-control-negative | Non-pipelined bounce baseline unchanged (~2.5-2.7ms) |
| h-robustness | P2P warm remains ~1.2-1.5ms; pipelined bounce within 30% of P2P |
| *iter-2 revises:* | |
| h-main | cudaHostAlloc + SPDK registration will achieve lower latency than non-pipelined bounce (20-40% gain) |
| h-control-negative | Non-pipelined bounce will be consistently slower than pipeline-v2 |
| h-robustness | P2P warm will outperform bounce and serve as lower bound; pipeline-v2 should approach it |

**Iter-1:** 17% gain, limited by per-chunk `connect_client()` bug (32×17μs = 544μs overhead).

| Condition | Throughput | Avg Latency |
|-----------|-----------|-------------|
| Sequential bounce (baseline) | 1440 MB/s | 2.78 ms |
| Pipelined bounce (new) | 1764 MB/s | 2.27 ms |
| P2P warm (reference) | 3082 MB/s | 1.30 ms |

**Iter-2:** Fixed with cudaHostAlloc + SPDK registration (pre-allocated at startup). 2.4-3x faster than sequential bounce.

| Condition | Avg Latency | Throughput |
|-----------|-------------|-----------|
| Pipeline-v2 (seed-1, device c3:00.0) | 4.96 ms | 806 MB/s |
| Pipeline-v2 (seed-2, device 64:00.0) | 4.97 ms | 805 MB/s |
| Sequential bounce (seed-1) | 15.10 ms | 265 MB/s |
| Sequential bounce (seed-2) | 12.03 ms | 333 MB/s |
| P2P warm (seed-1) | 5.04 ms | 794 MB/s |
| P2P warm (seed-2) | 1.93 ms | 2076 MB/s |

Pipeline-v2 is 2.4-3x faster than sequential bounce, confirming pipelining works. However, **P2P warm still wins** — on the clean seed (1.93ms vs 4.97ms, 2.6x faster). The congested seed shows near-parity (5.04ms vs 4.96ms) due to GDRCopy setup overhead under PCIe contention.

**Nous findings:** Iter-1 predicted 40-50% gain from pipelining — got 0% (small regression). Diagnosed that `cudaMemcpyAsync` was falling back to synchronous behavior because SPDK hugepage buffers aren't recognized as CUDA-pinned memory. Iter-2 switched to `cudaHostAlloc` + SPDK registration, predicted 20-40% gain — actually achieved 140-200% (2.4-3x). Speedup exceeded prediction because system was in degraded state (no hugepages for SPDK, slower baseline). P2P warm showed high cross-seed variance (5.04ms vs 1.93ms) — Nous attributed this to device/state variation.

**Nous alignment failure:** Nous marked h-main CONFIRMED by comparing pipeline vs bounce (correct direction), but the hypothesis asks whether pipelined bounce beats P2P — which it doesn't. Absolute latencies are higher than iter-1 due to system state (hugepage exhaustion; SPDK using slower DMA fallback path).

### 3. h8-dispatcher-p2p (2 iterations)

Isolated the P2P path advantage from the submission strategy (sequential ReadSync vs BatchSubmit).

| Arm | Prediction |
|-----|-----------|
| h-main | P2P warm (pre-pinned BAR1) will achieve lower latency and higher throughput than bounce |
| h-control-negative | P2P cold (per-request pin/unpin) will approach or exceed bounce latency |
| h-robustness | At 64 KiB chunks (64 commands), P2P still outperforms bounce but advantage narrows |

| Condition | Avg Latency | Throughput |
|-----------|-------------|-----------|
| P2P sequential | 1.58 ms | 2534 MB/s |
| Bounce sequential | 2.32 ms | 1724 MB/s |
| P2P batch | 1.10 ms | 3641 MB/s |
| Bounce batch | 2.73 ms | 1465 MB/s |

P2P-seq is 1.47x faster than bounce-seq. Surprise: bounce-batch avg (2.73ms) is WORSE than bounce-seq (2.32ms) due to NVMe queue saturation causing 10-11ms tail spikes. P2P-batch has no such problem (max-min spread <0.03ms).

**Nous findings:** Predicted P2P warm would have lower latency than bounce — confirmed (1.47x). Also confirmed P2P cold exceeds bounce (2.74x slower). Robustness arm at 64 KiB chunks showed P2P advantage narrows (2.47x→2.04x) as per-chunk command overhead increases — correctly predicted this narrowing.

### 4. h8-v0-vs-p2p (1 iteration)

First run through the actual system (gRPC → dispatcher → NVMe → GPU). Campaign constraints forced Nous to use `certus-server`.

| Arm | Prediction |
|-----|-----------|
| h-main | P2P sequential will achieve lower latency than bounce sequential for 4 MiB objects |
| h-control-negative | Staging-tier lookups (not evicted to SSD) will show identical latency for both paths |
| h-robustness | P2P will also be faster at 1 MiB, but relative advantage may be smaller |

| Condition | SSD Avg Latency | Throughput |
|-----------|----------------|-----------|
| Bounce (4 MiB) | 13,764 μs | 0.30 GB/s |
| P2P (4 MiB) | 18,372 μs | 0.23 GB/s |
| Bounce (1 MiB) | 5,228 μs | 0.20 GB/s |
| P2P (1 MiB) | 5,665 μs | 0.19 GB/s |

**P2P is 1.33x SLOWER than bounce through the dispatcher.** Root cause: cold pinning per request — the P2P implementation calls `prepare_memory_for_spdk()` on every lookup instead of maintaining a persistent staging pool. Per prior results, cold P2P is 2.74x slower than bounce.

Also notable: dispatcher bounce (13.7ms) is 6x slower than test binary bounce (2.3ms). The overhead comes from gRPC serialization, dispatch-map lookup, extent-manager resolution, per-segment DMA buffer allocation, and memory-tier management.

**Nous findings:** Predicted P2P would be faster — refuted. Diagnosed that sub-view DmaBuffers (`DmaBuffer::from_raw` at GPU base + chunk_offset) share the parent `spdk_mem_register`'d region but still incur per-request overhead. Control arm confirmed staging-tier lookups are identical (bypass `read_from_block_device`). Robustness arm at 1 MiB showed P2P disadvantage shrinks (1.33x→1.08x) — pinning overhead is proportionally larger for bigger objects.

### 5. h8-v1-vs-p2p (1 iteration)

Tested P2P vs pipelined bounce through `certus-server --dispatcher-version v1`. Required 200 turns ($9.14 executor cost alone; first attempt at 120 turns failed without data).

| Arm | Prediction |
|-----|-----------|
| h-main | P2P direct will achieve lower SSD-tier latency than pipelined bounce for 4 MiB objects |
| h-control-negative | At 4 KiB (1 chunk), P2P will NOT outperform bounce (setup overhead exceeds single cudaMemcpy savings) |

| Condition | SSD Avg Latency | SSD Min Latency | Throughput |
|-----------|----------------|----------------|-----------|
| Bounce v1 (4 MiB) | 12,969 μs | 11,424 μs | 0.32 GB/s |
| P2P (4 MiB) | 15,239 μs | 13,919 μs | 0.28 GB/s |
| Bounce v1 (4 KiB) | 460 μs | 244 μs | 0.01 GB/s |
| P2P (4 KiB) | 496 μs | 233 μs | 0.01 GB/s |

**P2P is 1.18x slower than bounce v1** — same direction as v0 (1.33x), slightly less severe. Same root cause: cold pinning per request via `prepare_memory_for_spdk()`. At 4 KiB (control-negative), difference is negligible (~8%), confirming the mechanism is in the bulk transfer path.

Notable: v1 bounce (12.97ms) is slightly faster than v0 bounce (13.76ms) for the same 4 MiB — the ring-buffer per-chunk approach has marginal benefit over v0's read-all-then-copy.

**Nous findings:** Predicted P2P would be faster — refuted. Diagnosed that per-lookup `prepare_memory_for_spdk` cost (`cudaIpcOpenMemHandle` + `spdk_mem_register`) exceeds the eliminated `cudaMemcpy` savings. Control arm at 4 KiB correctly predicted P2P wouldn't win at small sizes (setup overhead dominates), but got the magnitude ordering wrong — overhead fraction is actually larger at 4 MiB (17.5%) than 4 KiB (7.7%), opposite to prediction. Also discovered P2P path skips memory-tier promotion, making the benchmark comparison unfair (bounce: 1 SSD + 19 DRAM reads vs P2P: 20 SSD reads).

**Benchmark methodology flaw (affects both h8-v0-vs-p2p and h8-v1-vs-p2p):** The benchmark uses `--bench-iterations 20`, re-reading the same keys. Bounce promotes data to DRAM on first read, so iterations 2-20 are served from memory-tier (~328μs). P2P skips promotion, so all 20 iterations hit SSD (~13-15ms). Nous identified this accounts for ~75% of the measured slowdown — the "1.18x slower" result is mostly a caching artifact, not a DMA path comparison. A fair test requires `--bench-iterations 1` (first-access only). The h8-v1-pinned run (in progress) corrects this: Nous autonomously identified the flaw from prior findings, found the `--bench-only` flag, and designed its benchmark to use `--bench-iterations 1` for first-hit measurement.

### 6. h8-v0-pinned (budget exhausted — no data)

**GPU memory pinning flow (context for this and next run):** In the normal lookup path, the client allocates GPU memory via PyTorch (`torch.zeros(..., device="cuda:0")`), obtains a CUDA IPC handle, and sends it to the server over gRPC. The server calls `cudaIpcOpenMemHandle` to access that GPU memory, then calls `prepare_memory_for_spdk()` to register the buffer with SPDK for BAR1/DMA visibility (via GDRCopy `nvidia-peermem` mapping). All pinning and SPDK registration happens server-side — the Python client only provides a destination GPU buffer address. The "cold pinning" overhead (5-9ms per request) comes from `prepare_memory_for_spdk()` being called on every lookup. The pre-pinned approach moves this to server init: the server allocates its own GPU staging buffer, registers it once with SPDK, and reuses it across all lookups — then does a device-to-device copy from staging to the client's buffer.

**What Nous designed:** The Opus designer autonomously read prior findings from `.nous/h8-v0-vs-p2p/` (1.33x slower due to cold pinning), identified the root cause, and designed an experiment with three arms: (A) existing bounce path as baseline, (B) P2P with persistent staging (pre-pinned at init), (C) P2P without DRAM promotion (ablation to isolate raw transfer time). Implementation: add `p2p_staging: Mutex<Option<Arc<DmaBuffer>>>` field, call `prepare_memory_for_spdk()` once during `initialize()`, reuse across all lookups.

**What happened:** Nous chose to add fields to the shared `IpcHandle` struct in `idispatcher.rs`, which cascaded to both dispatcher versions, all benchmarks, integration tests, and the connector — 12 files changed in a 597-line patch. The executor spent 240 turns (the maximum) trying to fix build errors from this cross-cutting interface change and never produced a running benchmark. Cost: $14.10 ($2.56 design + $11.54 executor).

**Failure mode:** Implementation approach too invasive. A local change (P2P path only in v0, no shared interface modifications) would have been achievable within budget, but Nous chose to thread new fields through the entire interface layer.

### 7. h8-v1-pinned (queued)

Same as h8-v0-pinned but targeting dispatcher v1. Queued to run after h8-v0-pinned completes.

*(Results pending)*

---

## Key Findings

1. **Harness results don't predict system behavior** — P2P is 1.47x faster in isolation but 1.33x slower through the dispatcher due to integration overhead (cold pinning)
2. **Pre-pinned staging is mandatory** — cold P2P (per-request pin/unpin) is 2.74x slower than bounce; every harness run confirmed this but the first dispatcher implementation still got it wrong
3. **System overhead dominates** — dispatcher SSD lookup is 13.7ms vs harness 2.3ms; the DMA path optimization (saving ~0.7ms) is only 5% of total latency
4. **Pipelining works but doesn't beat P2P** — iter-2 confirms 2.4-3x gain over sequential bounce with proper cudaHostAlloc buffers, but P2P warm (1.93ms) still beats pipelined bounce (4.97ms) by 2.6x on a clean system
5. **Sequential submission is safer for bounce** — BatchSubmit causes tail amplification on bounce path; P2P is immune
6. **The hypothesis remains untested properly** — harness results (h8-pipelined iter-2) show pipelined bounce still loses to P2P warm (4.97ms vs 1.93ms). Dispatcher runs show P2P slower, but only because of cold pinning. No run has tested pipelined bounce against P2P with pre-pinned staging through the actual dispatcher.

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

The alignment failure repeated in h8-pipelined iter-2: Nous marked h-main CONFIRMED because pipeline-v2 beats sequential bounce (2.4-3x), but the hypothesis asks whether pipelined bounce beats *P2P* — which the same experiment's robustness arm shows it doesn't (4.97ms vs 1.93ms).

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
3. **Can pipelined bounce match P2P?** h8-pipelined iter-2 shows no — P2P warm (1.93ms) still 2.6x faster than pipelined bounce (4.97ms) in harness. The iter-1 prediction of near-parity (~820μs) was wrong; needs testing through the dispatcher where system overhead may equalize both paths
4. **Is the optimization worth pursuing?** If DMA path is only 5% of total dispatcher latency, the system overhead is the real target

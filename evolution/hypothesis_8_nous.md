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

| Run | Additional constraints | What Nous implemented | Key Result | Key Insight | Cost |
|-----|----------------------|----------------------|------------|-------------|------|
| h8-transfer-path | *(none — base hypothesis only)* | Nothing new — ran existing `--mode bounce`, `--mode p2p`, `--mode p2p-cold` in `gpu-p2p-server`; iter-2 added `--skip-nvme` flag to isolate copy phase | **Hypothesis not tested** (no pipelining exists, wrong binary); P2P 2x faster | P2P wins by eliminating 32 host→GPU copies (819μs total); NVMe read time is equal regardless of DMA target | $9.57 |
| h8-pipelined | + "Must use pipelined implementation, implement if not present" | New `--mode bounce-pipeline` in `gpu-p2p-server`: double-buffered ReadAsync + cudaMemcpyAsync with CUDA stream; iter-1 used cudaHostRegister (broken), iter-2 switched to cudaHostAlloc + spdk_mem_register (working) | **Hypothesis not tested on actual system**; iter-2: 2.4-3x over sequential bounce, still slower than P2P warm | GPU must own the memory from allocation (`cudaHostAlloc`) for async DMA to work — registering existing SPDK buffers with CUDA falls back to synchronous | $15.49 |
| h8-dispatcher-p2p | *(base hypothesis, pointed to dispatcher v1)* | New `--mode p2p-seq` and `--mode bounce-seq` in `gpu-p2p-server`: sequential ReadSync variants (vs existing parallel-submit modes) to isolate DMA path from submission strategy | **Hypothesis not tested on actual system**; P2P-seq 1.47x faster in test binary | Submitting all 32 NVMe reads in parallel causes 10ms tail spikes for bounce (buffer pool contention) but not P2P | $10.41 |
| h8-v0-vs-p2p | + **"Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server"** --dispatcher-version v0 | P2P read path in `dispatcher/v0/src/lib.rs`: added `cuda_ipc_handle_bytes` to IpcHandle, calls `prepare_memory_for_spdk()` per lookup, reads NVMe chunks directly into GPU DMA buffer (323-line patch, 4 files) | **Tested on actual system**; P2P 1.33x **slower** — cold pinning kills advantage | Per-request GPU memory registration (5-9ms) negates the DMA path savings (~1ms); must register once and reuse | $7.66 |
| h8-v1-vs-p2p | + **"Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server"** --dispatcher-version v1 | P2P read path in `dispatcher/v1/src/pipeline.rs`: same approach as v0 — per-request `prepare_memory_for_spdk()`, reads into GPU buffer, skips memory-tier promotion (6 files including pipeline.rs) | **Tested on actual system**; P2P 1.18x **slower** — same cold pinning issue | v1's "pipelined" bounce barely faster than v0 sequential (13.0ms vs 13.8ms) — no actual overlap between read and copy stages | ~$16.46 |
| h8-v0-pinned | + "even with pre-pinned GPU memory" --dispatcher-version v0 | Attempted: added `cuda_ipc_handle_bytes: Option<Vec<u8>>` to shared `IpcHandle` in `idispatcher.rs` + P2P staging pool in v0 — cascaded to 12 files (597-line patch), never compiled | **Budget exhausted** (240 turns), no data | Cross-cutting interface changes exceed Nous budget; keep implementations local | $14.10 |
| h8-v1-pinned | + "even with pre-pinned GPU memory" --dispatcher-version v1 | Iter-1: GPU DMA buffer cache in dispatcher v1 — registers GPU memory with NVMe once per client, reuses across all lookups. Iter-2: added parallel NVMe reads (32 concurrent) into the pre-pinned GPU buffer | **Tested on actual system**; iter-1: P2P 2.02x **faster** (9.3ms vs 18.8ms); iter-2: 778μs (likely NVMe controller cache artifact) | Python client works fine for P2P; server-side GPU buffer caching handles all registration — no native client needed | ~$12 |
| **Total** | | | | | **~$85.69** |

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

## Key Findings

1. **Harness results don't predict system behavior** — P2P is 1.47x faster in isolation but 1.33x slower through the dispatcher due to integration overhead (cold pinning)
2. **Pre-pinned staging is mandatory** — cold P2P (per-request pin/unpin) is 2.74x slower than bounce; every test confirmed this but the first dispatcher implementation still got it wrong
3. **System overhead dominates** — dispatcher SSD lookup is 13.7ms vs isolated test binary 2.3ms; the DMA path optimization (saving ~0.7ms) is only 5% of total latency
4. **Pipelining works but doesn't beat P2P** — iter-2 confirms 2.4-3x gain over sequential bounce with proper cudaHostAlloc buffers, but P2P warm (1.93ms) still beats pipelined bounce (4.97ms) by 2.6x on a clean system
5. **Sequential NVMe submission is safer for bounce** — submitting all 32 reads in parallel causes 10ms tail spikes on bounce path (buffer pool exhaustion); P2P is immune
6. **Pre-pinned P2P confirmed faster through the dispatcher** — h8-v1-pinned resolved the cold-pinning issue; P2P with persistent staging is 2.02x faster than sequential bounce through certus-server (9.3ms vs 18.8ms). The hypothesis (bounce wins) is refuted for sequential bounce. Remaining question: can *true pipelined* bounce (overlapped NVMe read + GPU copy) close the gap? No run has tested this yet — campaigns `h8-evolve-v0-pipelined` and `h8-v1-true-pipeline` are queued for this.

---

## Nous Assessment

**Strengths:**
- Code discovery: found all transfer modes, identified GDRCopy overhead, noted MDTS constraint, correctly mapped dispatcher architecture
- Experiment design: clean controls, reproducible conditions, correct measurement protocols
- Instrumentation: high-quality latency decomposition in iter-2 (per-phase breakdown is exactly what we needed)
- Cross-campaign learning: autonomously read prior experiment data from `.nous/` directories and compounded knowledge (h8-v1-pinned fixed cold pinning because it read h8-v1-vs-p2p findings)

**What Nous found on its own (no hints given):**
- `gpu-p2p-server` binary and all its transfer modes
- `test_client.py` benchmark and how to use it for latency measurement
- `prepare_memory_for_spdk()` as the GPU registration mechanism
- Cold pinning overhead as root cause when P2P was slower through dispatcher
- The DRAM promotion confound in multi-iteration benchmarks (discovered after seeing prior run failures)
- `cudaHostAlloc` + `spdk_mem_register` as the correct combination for async DMA (after iter-1 failure with `cudaHostRegister`)

**What Nous successfully implemented (no code hints given):**
- P2P read path in dispatcher v0 and v1 — NVMe reads directly into GPU-registered buffer
- GPU DMA buffer cache — one-time registration at init, reused across all lookups (solved cold pinning)
- Double-buffered async pipeline (`cudaHostAlloc` + CUDA streams) for overlapped NVMe read + GPU copy
- Sequential vs parallel NVMe submission modes for controlled comparison
- `--skip-nvme` flag to isolate the copy-phase bottleneck
- Server-side IPC handle caching — figured out Python client works fine, no native client needed

**What Nous never found or attempted:**
- v1's pipelining is fake (sequential per-chunk, no overlap) — never identified across 7 runs despite reading `pipeline.rs` multiple times
- True pipelining as the key to testing the hypothesis — never attempted overlapping NVMe reads with GPU copies through the dispatcher
- That `--bench-iterations 20` creates an unfair comparison for paths with different promotion behavior — only discovered after 4 runs used it
- The NVMe controller cache invalidation problem — accepted "SSD reads" at face value without questioning whether data was truly cold
- Questioning `test_client.py`'s benchmark methodology — when results were suspicious (SSD faster than memory, multi-iteration promotion), Nous tweaked parameters rather than recognizing the measurement design itself was flawed
- Questioning whether the hypothesis could be saved (true pipelining) after showing sequential bounce loses — pivoted to making P2P faster instead

**Failure modes:**
1. No hypothesis-to-experiment alignment check (tested a different question than asked)
2. Path of least resistance (used isolated test binary until explicitly constrained to use the full system)
3. Implementation bugs become blockers (`connect_client()` per chunk, cold pinning)
4. Budget exhaustion on complex implementations (v1 P2P: 120 turns, no data; v0-pinned: 240 turns, interface change cascaded to 12 files)
5. Uncritical code discovery — finds existing tools and uses them without evaluating appropriateness (used `gpu-p2p-server` in 4 runs, used `--bench-iterations 20` without considering caching effects, accepted impossible 778μs SSD result at face value)
6. Explores the winning path deeper rather than strengthening the losing path — once P2P won, Nous kept optimizing P2P instead of giving bounce its best shot (true pipelining)

**Recommendations for Nous development:**
1. Add `constraints` field to campaign schema (hard rules validated before execution)
2. Weight keywords in hypothesis — flag if experiment doesn't address them
3. Hypothesis-to-experiment alignment gate (reject bundle if it doesn't test what's stated)
4. Sanity-check gate on results (flag if measurements violate physical constraints, e.g. SSD faster than DRAM)

---

---

## Detailed Results

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

### 7. h8-v1-pinned (in progress)

Same hypothesis as h8-v0-pinned but targeting dispatcher v1. Where h8-v0-pinned failed (12-file cascade, budget exhausted), this run succeeded — Nous learned from the failure and kept the interface change local.

**Arm predictions (iter-1):**

| Arm | Prediction | Actual |
|-----|-----------|--------|
| h-main | P2P persistent faster than bounce (20 iter) | **Confounded** — bounce promotes to DRAM after iter-1 (343μs avg), P2P stays on SSD (7,984μs avg). Invalid comparison. |
| h-robustness | P2P persistent faster on first cold hit (1 iter) | **CONFIRMED** — P2P 9,272μs vs bounce 18,752μs → **2.02x faster** |

**Iter-1 results:**

| Condition | Iterations | SSD-tier Avg (μs) | GB/s | Notes |
|-----------|-----------|-------------------|------|-------|
| Bounce baseline | 20 | 1,299 | 3.23 | Misleading — iter 2-20 served from DRAM (332μs) |
| P2P persistent | 20 | 7,984 | 0.53 | All 20 iters hit SSD (no promotion) |
| Bounce baseline | 1 | 18,752 | 0.22 | Fair cold-read measurement |
| **P2P persistent** | **1** | **9,272** | **0.45** | **Fair cold-read — 2.02x faster** |

**What Nous did right:**
- Learned from h8-v0-pinned failure: kept `cuda_ipc_handle_bytes` field addition to `IpcHandle` in `idispatcher.rs` but avoided cascading changes by making it `Option<Vec<u8>>` (all existing callers pass `None`)
- Autonomously identified the caching confound from prior findings (RP-6) and designed h-robustness arm with `--bench-iterations 1` for fair comparison
- Used `--bench-only` flag (avoiding the functional test bug that blocked earlier runs)
- Correctly rejected h-main as "regime error" in findings — shows Nous can self-diagnose invalid measurements
- Cached `prepare_memory_for_spdk` result keyed by 64-byte CUDA IPC handle — exactly the fix needed for cold pinning

**What Nous did wrong:**
- After confirming P2P is 2x faster, designed iter-2 to make P2P *even faster* with BatchSubmit (parallel NVMe reads at QD=32) instead of strengthening bounce (true pipelining) to see if the hypothesis could be saved. This is failure mode #6 — exploring the winning path deeper rather than testing the losing path's best case.
- Used `test_client.py`'s overfill-and-evict approach (populate 69 objects into 64-slot pool, forcing 5 evictions to SSD) to measure "cold" SSD reads. But this only evicts from certus's memory-tier — data remains hot in the NVMe controller's internal DRAM cache. With only 5 evictions and QD=32 parallel reads, the controller serves from its write-back cache, not flash. This produced the impossible 778μs "SSD" read — faster than memory-tier (2,289μs).

**Iter-2 results:**

| Condition | SSD-tier (μs) | GB/s | Notes |
|-----------|--------------|------|-------|
| A: Bounce sequential | 21,247 | 0.20 | Baseline |
| B: P2P sequential | 15,160 | 0.28 | 1.4x faster (down from iter-1's 2.02x — system state variance) |
| C: P2P + BatchSubmit QD=32 | **778** | **5.39** | **Likely invalid** — SSD faster than memory-tier (physically impossible for true cold reads); NVMe controller cache artifact |
| D: Bounce + BatchSubmit | 6,878 | 0.61 | 4/5 runs failed with ENOMEM (rc=-12); bounce buffer pool exhaustion |

**Iter-2 analysis:**
- The headline 27x speedup (condition C) is almost certainly a measurement artifact. SSD-tier latency (778μs) being lower than memory-tier (2,289μs) violates the storage hierarchy — this means the "SSD reads" were served from NVMe controller DRAM, not flash media.
- Condition D confirms that bounce can't support high QD: the DMA ring buffer pool (4 buffers) exhausts at QD=32, causing `spdk_nvme_ns_cmd_read` to return ENOMEM. P2P avoids this because it writes directly to a single pre-pinned GPU buffer without needing host-side DMA buffers per outstanding read.
- Condition B (1.4x) is lower than iter-1's 2.02x — likely system state variance (different eviction count: 5 vs 10, potentially warmer NVMe cache from condition A running first).
- Nous reported "CONFIRMED with extreme margin" without flagging that SSD < memory-tier is physically impossible. This is failure mode #5 (uncritical) — accepts measurement results at face value.

**Cost:** ~$12 (design $2.56 + executor ~$9.50)

---

## Conclusion: What Should Certus Use?

**Answer to the original hypothesis:** Pipelined bounce is NOT faster than P2P DMA — at least not with sequential bounce. P2P with pre-pinned staging is 2.02x faster (9.3ms vs 18.8ms) through the actual dispatcher for 4MiB cold SSD reads. The hypothesis is refuted.

**What Certus should implement:**
- **P2P with persistent GPU staging** as the primary SSD→GPU path. Register the GPU buffer once per client connection, reuse across all lookups. The Python client + server-side IPC handle caching works — no native client needed.
- **Parallel NVMe reads (QD=32)** combined with P2P for maximum throughput. The principle is sound even though the 778μs measurement was a cache artifact — parallel submission eliminates the sequential 32×single-read bottleneck.
- **Keep bounce as fallback** for systems without GPU BAR1 visibility or when P2P registration fails.

**What remains unproven:**
- Whether true pipelined bounce (overlapped NVMe read + GPU copy with async streams) can compete with P2P through the dispatcher. The `h8-evolve-v0-pipelined` and `h8-v1-true-pipeline` campaigns are testing this now. If pipelining closes the gap, it may be preferable since it doesn't require GPU BAR1/GDRCopy setup.
- Where the ~7ms system overhead lives (P2P is 9.3ms through dispatcher vs 2.3ms in isolated test binary). Profiling the gRPC/extent-manager/allocation path could yield larger gains than DMA path optimization.

**Practical impact:** P2P saves 9.5ms per 4MiB cold lookup. For a 70B model (~140GB KV-cache, 35k lookups at 4MiB), that's ~330s saved on a full cold-start restore. The system overhead (~7ms constant per lookup regardless of path) is the larger target — reducing it would benefit both P2P and bounce paths equally.

## Next Hypotheses

1. **True pipelined bounce vs P2P** (h8-evolve-v0-pipelined running, h8-v1-true-pipeline queued) — can overlapped NVMe reads + GPU copies close the 2x gap? If pipelining achieves near-perfect overlap, the effective bounce latency could drop from 18.8ms to ~10ms (NVMe + GPU copy in parallel rather than sequential). Still likely slower than P2P (9.3ms) but the gap narrows — and pipelining doesn't require GPU BAR1/GDRCopy infrastructure.
2. **System overhead is the real bottleneck** — P2P saves 9.5ms on the DMA path, but 7ms of system overhead (gRPC, extent-manager, allocation) affects both paths equally. Profiling and reducing this constant overhead may yield more throughput gain than any DMA path optimization.
3. **Parallel NVMe + P2P at scale** — QD=32 parallel reads into pre-pinned GPU buffer. The principle is sound (eliminates sequential 32×single-read bottleneck) but needs proper measurement with cold NVMe controller state.


# Hypothesis 8 — Nous Experiment Analysis

## Objective

Certus needs to move KV-cache data from NVMe SSDs to GPU memory as fast as possible. There are two paths: bounce through host RAM (with the option to pipeline/overlap stages) or direct P2P DMA from SSD to GPU. We used Nous to find out which is faster and whether it can design and implement what's missing to test this hypothesis.

**Hypothesis:** Pipelined bounce-buffer (SSD→CPU→GPU) transfers outperform direct SSD→GPU P2P DMA for 4 MiB objects at 128 KiB NVMe chunk size.

**Meta-question:** With minimal direction, can Nous autonomously answer this? If not, how explicit do we need to be — and at what point does it produce useful implementations?

The 8 campaigns below progressively increase the level of direction given to Nous, from "just the research question" to "specific implementation strategy." This reveals what Nous can figure out on its own vs what it needs to be told.

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
                 ↕ no intermediate copy, but requires GPU BAR1 visibility + pre-pinned registration
```

---

## Key Result

**Hypothesis partially confirmed after 10 progressively directed campaigns.** Pipelined bounce with pre-allocated buffers (9.7ms) outperforms cold P2P (20.1ms) by 2.08x — cold pinning per request kills P2P. But when P2P is pre-pinned (one-time registration), performance converges: pre-pinned P2P (~9.3ms) reaches near-parity with pipelined bounce (~9.7ms, measured in separate runs). System overhead (gRPC + dispatcher, 15-25ms) dwarfs both paths and is the actual bottleneck.

The final campaign (h8-v1-optimized-pipeline) confirmed the real lever: **parallel NVMe reads**, not GPU copy overlap. When pointed at `gpu-bb-vs-p2p` as reference, Nous correctly identified that v1 serializes 32 ReadSync calls (~19.2ms floor) and replaced them with parallel ReadAsync + primed ring. Result: v1 SSD-tier latency dropped from ~15.4ms to ~7.9ms (2x). The `gpu-bb-vs-p2p` ceiling with ring_size=32 is ~1.3ms — further improvement requires increasing queue depth.

Nous never recognized that v1's "pipelining" has no actual overlap, and never attempted true async pipelining until explicitly told to. But once directed, it implemented overlap in both v0 and v1 and — through iterative experimentation — diagnosed what it believes is the root cause: NVMe read dominates GPU copy by 10-100x, making overlap irrelevant (iter-1 tried `cudaHostRegistered` → only 10% → iter-2 tried `cudaHostAlloc` → 0% → diagnosed NVMe dominance). The actual performance benefit in v0 comes from buffer pre-allocation (eliminating 32× `DmaBuffer::new`), not overlap. The v1 patch failed to pre-allocate (allocates `cudaHostAlloc` per call instead), which is why it showed zero improvement. The v0 patch outperforms cold P2P (9.7ms vs 20.1ms) but skips memory-tier promotion — repeat lookups still hit SSD. The two dispatchers serve different purposes and were never compared head-to-head.

---

## Experiment Overview

**Base hypothesis (given to all runs):**
> Using a bounce buffer SSD→CPU→GPU with pipelined transfers is faster than direct SSD→GPU for transfer of 4 MiB broken into a stream of 128 KiB transfers.

**What exists in the codebase:**
- **Dispatcher:** The component inside `certus-server` responsible for SSD→GPU data movement. Handles NVMe reads, memory-tier promotion, and GPU copies. `certus-server` is the full system (gRPC + dispatcher + extent-manager + memory-tier). The hypothesis should be tested through `certus-server` to exercise the dispatcher in context.
  - **v0:** Reads all 128 KiB chunks from SSD into a contiguous host DRAM buffer (bounce), then does a single `dma_copy_to_device` to GPU. No memory-tier — data goes SSD→DRAM→GPU and that's it. No pipelining, no P2P.
  - **v1:** Reads each chunk into a ring of 4 DMA buffers, copies to memory-tier slot (DRAM cache for future lookups) AND to GPU. The memory-tier is the whole point of v1 — repeat lookups are served from DRAM. Despite the name "pipelined," it's sequential per-chunk (no overlap between read and copy stages). No P2P.
- **`gpu-p2p-server`:** Standalone test binary for validating P2P DMA in isolation. Talks directly to NVMe + GPU, bypasses entire dispatcher stack. Has bounce/P2P/P2P-cold modes but no pipelining.
- **Test clients (pre-existing):**
  - `apps/certus-server/python-client/test_client.py` — gRPC benchmark client for `certus-server`. Populates objects, forces eviction to SSD, then measures per-object lookup latency (`time.perf_counter()` around `stub.Lookup`) and throughput (`object_size / latency`). Reports avg/min/max latency (μs) and GB/s for both memory-tier and SSD-tier lookups.
  - `components/gpu-services/v0/tests/gpu_client_p2p.py` — Unix socket client for `gpu-p2p-server`. Sends transfer requests, measures per-transfer latency and throughput (MB/s). Reports avg/min/max latency (ms).
- **Neither dispatcher has P2P or true pipelining.** We want to see whether Nous can discover this gap and implement what's missing to properly test the hypothesis.

| Run | Additional constraints | What Nous implemented | Hypothesis verdict | Key Insight | Cost |
|-----|----------------------|----------------------|------------|-------------|------|
| h8-transfer-path | *(none — base hypothesis only)* | Nothing new — ran existing `--mode bounce`, `--mode p2p`, `--mode p2p-cold` in `gpu-p2p-server`; iter-2 added `--skip-nvme` flag to isolate copy phase | **Hypothesis not aligned** — no pipelining, wrong binary. P2P 2x faster in isolation | P2P wins by eliminating 32 host→GPU copies (819μs total); NVMe read time is equal regardless of DMA target | $9.57 |
| h8-pipelined | + "Must use pipelined implementation, implement if not present" | New `--mode bounce-pipeline` in `gpu-p2p-server`: double-buffered ReadAsync + cudaMemcpyAsync with CUDA stream; iter-1 used cudaHostRegister (broken), iter-2 switched to cudaHostAlloc + spdk_mem_register (working) | **Hypothesis not aligned** — wrong binary. Pipelining 2.4-3x over sequential, still slower than P2P warm | GPU must own the memory from allocation (`cudaHostAlloc`) for async DMA to work — registering existing SPDK buffers with CUDA falls back to synchronous | $15.49 |
| h8-dispatcher-p2p | *(base hypothesis, pointed to dispatcher v1)* | New `--mode p2p-seq` and `--mode bounce-seq` in `gpu-p2p-server`: sequential ReadSync variants (vs existing parallel-submit modes) to isolate DMA path from submission strategy | **Hypothesis not aligned** — wrong binary, no pipelining. P2P-seq 1.47x faster in isolation | Submitting all 32 NVMe reads in parallel causes 10ms tail spikes for bounce (buffer pool contention) but not P2P | $10.41 |
| h8-v0-vs-p2p | + **"Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server"** --dispatcher-version v0 | P2P read path in `dispatcher/v0/src/lib.rs`: added `cuda_ipc_handle_bytes` to IpcHandle, calls `prepare_memory_for_spdk()` per lookup, reads NVMe chunks directly into GPU DMA buffer (323-line patch, 4 files) | **Hypothesis not aligned** — no pipelining, cold P2P. Sequential bounce 1.33x faster than cold P2P | Per-request GPU memory registration (5-9ms) negates the DMA path savings (~1ms); must register once and reuse | $7.66 |
| h8-v1-vs-p2p | + **"Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server"** --dispatcher-version v1 | P2P read path in `dispatcher/v1/src/pipeline.rs`: same approach as v0 — per-request `prepare_memory_for_spdk()`, reads into GPU buffer, skips memory-tier promotion (6 files including pipeline.rs) | **Hypothesis not aligned** — no pipelining, cold P2P. Sequential bounce 1.18x faster than cold P2P | v1's "pipelined" bounce barely faster than v0 sequential (13.0ms vs 13.8ms) — no actual overlap between read and copy stages | ~$16.46 |
| h8-v0-pinned | + "even with pre-pinned GPU memory" --dispatcher-version v0 | Attempted: added `cuda_ipc_handle_bytes: Option<Vec<u8>>` to shared `IpcHandle` in `idispatcher.rs` + P2P staging pool in v0 — cascaded to 12 files (597-line patch), never compiled | **No data** — budget exhausted (240 turns) | Cross-cutting interface changes exceed Nous budget; keep implementations local | $14.10 |
| h8-v1-pinned | + "even with pre-pinned GPU memory" --dispatcher-version v1 | Iter-1: GPU DMA buffer cache in dispatcher v1 — registers GPU memory with NVMe once per client, reuses across all lookups. Iter-2: added parallel NVMe reads (32 concurrent) into the pre-pinned GPU buffer | **Hypothesis not aligned** — no pipelining. Sequential bounce refuted: pre-pinned P2P 2.02x faster (9.3ms vs 18.8ms) | Python client works fine for P2P; server-side GPU buffer caching handles all registration — no native client needed | ~$12 |
| h8-evolve-v0-pipelined | + **"Do NOT use gpu-p2p-server. Overlap NVMe reads with GPU copies (true pipelining). Do NOT reference v1."** --dispatcher-version v0 | Iter-1: double-buffered `cudaHostAlloc` + `cudaMemcpyAsync` on CUDA streams — reads chunk N+1 while copying chunk N to GPU. Iter-2: tried BatchSubmit QD=32 (all chunks in parallel) — no improvement due to gRPC/connect_client overhead | **Confirmed** — pipelined bounce (9.7ms) 2.08x faster than P2P (20.1ms). Note: P2P here uses cold pinning (no pre-pinning was implemented); vs pre-pinned P2P (9.3ms from h8-v1-pinned) they're tied | True overlap works but only ties P2P; gRPC round-trip (~15-25ms) swamps NVMe-level optimizations — the bottleneck is infrastructure, not DMA | $15.59 |
| h8-v1-true-pipeline | + **"Do NOT use gpu-p2p-server. Fix v1 to truly overlap reads and copies."** --dispatcher-version v1 | Iter-1: `cudaMemcpyAsync` on `cudaHostRegistered` mmap'd ring buffers (10% gain). Iter-2: switched to `cudaHostAlloc` — no improvement (NVMe read 10-100x longer than GPU copy, nothing to hide). P2P arm uses cached pre-pinning | **Refuted** — pipelining gives 0-10% gain (within noise); P2P pre-pinned 19-23% faster. NVMe read dominates at 128 KiB chunks — GPU copy overlap is irrelevant | NVMe read (~600μs/chunk) dominates GPU copy (~5-50μs/chunk) by 10-100x; overlapping saves <5%, below measurement noise. Contradicts v0's 2x (that gain was from eliminating DmaBuffer::new, not from overlap) | $19.92 |
| h8-v1-optimized-pipeline | + **"Use apps/gpu-bb-vs-p2p/ as reference for optimized pipeline."** --dispatcher-version v1 | Parallel ReadAsync with primed ring (QD=2) + `cudaMemcpyAsync` on 2 alternating CUDA streams. Correctly identified sequential ReadSync as the bottleneck (32× ~600μs = 19.2ms floor). Timed out before completing ablation arm | **Confirmed** — parallel NVMe reads cut v1 SSD-tier latency ~2x (15.4ms → 7.9ms). Ablation (NVMe parallelism only, no async GPU) not completed due to timeout | Parallel NVMe submission is the real lever — even with ring_size=2, parallelizing reads halves latency. With gpu-bb-vs-p2p's ring_size=32, ceiling is ~1.3ms (12x further improvement possible). Campaign timed out at 30min budget | ~$2.85 |
| **Total** | | | | | **~$124.05** |

All runs used Opus for design, Sonnet for execute_analyze.

The first 4 runs (h8-transfer-path, h8-pipelined, h8-evolve-v0, h8-dispatcher-p2p) all used `gpu-p2p-server` — a standalone test binary that exists in the repo for validating P2P DMA in isolation. It talks directly to NVMe + GPU, bypassing the entire dispatcher stack (gRPC, extent-manager, memory-tier, dispatch-map). Nous found this binary on its own while exploring the codebase and decided to use it instead of certus-server because it's simpler to instrument and doesn't require understanding the full system.

Only after adding explicit constraints to the campaign description ("Do NOT use gpu-p2p-server. All benchmarks MUST run through certus-server.") did the last 2 runs (h8-v0-vs-p2p, h8-v1-vs-p2p) test through the actual system. This revealed that isolated results were misleading — P2P goes from 1.47x faster in the test binary to 1.33x slower through the dispatcher.

The pinned campaigns (h8-v0-pinned, h8-v1-pinned) add only "even with pre-pinned GPU memory" to the research question — no implementation hints about persistent pools or amortized pinning. The Opus designer autonomously discovered prior experiment data in `.nous/h8-v0-vs-p2p/` and `.nous/h8-v1-vs-p2p/`, read their findings (1.33x slower due to cold pinning overhead), and designed the new experiment specifically to amortize the pin cost: one-time GPU buffer preparation at init, reused across all lookups. This demonstrates that Nous can compound knowledge across campaigns when prior results are accessible in the repository.

The evolve campaigns (h8-evolve-v0-pipelined, h8-v1-true-pipeline) go one step further: the research question explicitly states "overlap NVMe reads with GPU copies (true pipelining)." This is the most direction given to any run — naming the implementation strategy, not just the goal. With this level of guidance, Nous built working implementations in both dispatchers and correctly diagnosed why the hypothesis fails: NVMe read (~600μs/chunk) dominates GPU copy (~5-50μs/chunk) by 10-100x, making overlap irrelevant at 128 KiB chunks. The h8-evolve-v0-pipelined 2.02x gain turned out to be from eliminating `DmaBuffer::new` allocations, not from async overlap — confirmed when h8-v1-true-pipeline's `cudaHostAlloc` pipeline showed zero improvement. This demonstrates: even when the hypothesis is wrong, Nous at this direction level produces correct diagnostics.

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

### How much direction does Nous need?

| Direction level | What was given | Result | Runs |
|----------------|---------------|--------|------|
| Research question only | "Is pipelined bounce faster than P2P?" | Tests wrong binary, never reaches dispatcher | h8-transfer-path, h8-pipelined, h8-dispatcher-p2p (~$35) |
| System constraint | + "Use certus-server, not gpu-p2p-server" | Correct system, but naive implementation (cold pinning makes P2P slower) | h8-v0-vs-p2p, h8-v1-vs-p2p (~$24) |
| Design hint | + "Pre-pinned GPU memory" | Solves integration issue, produces valid comparison | h8-v1-pinned (~$12) |
| Implementation strategy | + "Overlap NVMe reads with GPU copies" | Builds pipeline, 2x gain (from buffer reuse, not overlap); correctly diagnoses NVMe read dominance | h8-evolve-v0-pipelined, h8-v1-true-pipeline (~$35) |
| Reference implementation | + "Use gpu-bb-vs-p2p as reference pattern" | Correctly identifies sequential NVMe as bottleneck, implements parallel ReadAsync, achieves 2x on v1 | h8-v1-optimized-pipeline (~$3) |

**Answer:** Nous needs the system constraint (where to test) and a design hint (what approach to take). It cannot make architectural decisions autonomously but executes well once pointed in a direction. Implementation details — buffer management, CUDA stream setup, memory registration — it figures out on its own. At the "implementation strategy" level, Nous also correctly diagnoses *why* things don't work (NVMe read dominance, cudaHostRegister fallback) — valuable even when the hypothesis is refuted. Providing a reference implementation (`gpu-bb-vs-p2p`) as a pattern to follow was the most cost-effective approach: $3 for a 2x gain vs $35 when given only the strategy name.

### Domain findings

1. **Isolated test results don't predict system behavior** — P2P is 1.47x faster in isolation but 1.33x slower through the dispatcher due to integration overhead (cold pinning)
2. **Pre-pinned staging is mandatory** — cold P2P (per-request pin/unpin) is 2.74x slower than bounce; every test confirmed this but the first dispatcher implementation still got it wrong
3. **System overhead dominates** — dispatcher SSD lookup is 13.7ms vs isolated test binary 2.3ms; the DMA path optimization (saving ~0.7ms) is only 5% of total latency
4. **Pipelining gains are from buffer reuse, not overlap** — h8-evolve-v0-pipelined showed 2.02x gain against v0 (which allocates 32× `DmaBuffer::new` per lookup, ~300μs each = ~9.6ms overhead). h8-v1-true-pipeline showed 0% gain against v1 (which already has a 4-buffer ring, only ~1.2ms allocation overhead — within noise). GPU copy overlap saves <5% in both cases because NVMe read dominates by 10-100x at 128 KiB chunks
5. **Sequential NVMe submission is safer for bounce** — submitting all 32 reads in parallel causes 10ms tail spikes on bounce path (buffer pool exhaustion); P2P is immune
6. **Pre-pinned P2P is faster than sequential bounce, tied with pipelined bounce** — P2P with persistent staging is 2.02x faster than sequential bounce (9.3ms vs 18.8ms). But pipelined bounce with pre-allocated buffers achieves ~9.7ms — essentially tied with P2P. Cold P2P (per-request pinning) is slower than both bounce variants (20.1ms)
7. **gRPC + connect_client overhead dominates** — iter-2 of h8-evolve-v0-pipelined shows BatchSubmit QD=32 provides zero improvement because gRPC round-trip + channel setup (~15-25ms) swamps NVMe read time (~0.8ms). DMA path optimizations are invisible at the end-to-end benchmark level

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

**What Nous successfully implemented:**

*No guidance (figured out on its own):*
- P2P read path in dispatcher v0 and v1 — NVMe reads directly into GPU-registered buffer
- Sequential vs parallel NVMe submission modes for controlled comparison
- `--skip-nvme` flag to isolate the copy-phase bottleneck
- Server-side IPC handle caching — figured out Python client works fine, no native client needed
- `cudaHostAlloc` + `spdk_mem_register` as the correct memory combination (after iter-1 failure with `cudaHostRegister`)

*With design hint ("pre-pinned GPU memory"):*
- GPU DMA buffer cache — one-time registration at init, reused across all lookups (solved cold pinning)

*With explicit implementation strategy ("overlap NVMe reads with GPU copies"):*
- Double-buffered async pipeline (`cudaHostAlloc` + CUDA streams) for overlapped NVMe read + GPU copy — correctly implemented, but 2.02x gain was from buffer pre-allocation, not overlap (h8-v1-true-pipeline confirmed overlap saves <5%)

**What Nous never found or attempted:**
- v1's pipelining is fake (sequential per-chunk, no overlap) — never identified across 7 runs despite reading `pipeline.rs` multiple times
- True pipelining as the key to testing the hypothesis — never attempted overlapping NVMe reads with GPU copies through the dispatcher
- That `--bench-iterations 20` creates an unfair comparison for paths with different promotion behavior
- The NVMe controller cache invalidation problem — accepted "SSD reads" at face value without questioning whether data was truly cold
- Questioning `test_client.py`'s benchmark methodology — when results were suspicious (SSD faster than memory, multi-iteration promotion), Nous tweaked parameters rather than recognizing the measurement design itself was flawed
- Questioning whether the hypothesis could be saved (true pipelining) after showing sequential bounce loses — pivoted to making P2P faster instead
- Sustained throughput testing — every campaign lists "sustained lookup throughput over N sequential lookups" as a metric, but most runs only benchmarked 1 key × 1 iteration (h8-v1-true-pipeline autonomously chose 10 objects — likely learned from prior run variance)

**Failure modes:**
1. No hypothesis-to-experiment alignment check (tested a different question than asked)
2. Path of least resistance (used isolated test binary until explicitly constrained to use the full system)
3. Implementation bugs become blockers (`connect_client()` per chunk, cold pinning)
4. Budget exhaustion on complex implementations (v1 P2P: 120 turns, no data; v0-pinned: 240 turns, interface change cascaded to 12 files)
5. Uncritical code discovery — finds existing tools and uses them without evaluating appropriateness (used `gpu-p2p-server` in 4 runs, used `--bench-iterations 20` without considering caching effects, accepted impossible 778μs SSD result at face value)
6. Explores the winning path deeper rather than strengthening the losing path — once P2P won, Nous kept optimizing P2P instead of giving bounce its best shot (true pipelining)
7. Iter-2 abandons working iter-1 approach — h8-evolve-v0-pipelined iter-1 achieved 2.02x with double-buffering, but iter-2 switched to BatchSubmit QD=32 (completely different strategy) and got no improvement. Never investigated why iter-1 worked or tried to refine it

**Recommendations for Nous development:**
1. Add `constraints` field to campaign schema (hard rules validated before execution)
2. Weight keywords in hypothesis — flag if experiment doesn't address them
3. Hypothesis-to-experiment alignment gate (reject bundle if it doesn't test what's stated)

**Recommendations for the test evaluator:**
1. Sanity-check gate on results (flag if measurements violate physical constraints, e.g. SSD faster than DRAM)

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

**Benchmark methodology flaw (affects both h8-v0-vs-p2p and h8-v1-vs-p2p):** The benchmark uses `--bench-iterations 20`, re-reading the same keys. Bounce promotes data to DRAM on first read, so iterations 2-20 are served from memory-tier (~328μs). P2P skips promotion, so all 20 iterations hit SSD (~13-15ms). Nous identified this accounts for ~75% of the measured slowdown — the "1.18x slower" result is mostly a caching artifact, not a DMA path comparison. A fair test requires `--bench-iterations 1` (first-access only). The h8-v1-pinned run corrected this: Nous autonomously identified the flaw from prior findings, found the `--bench-only` flag, and designed its benchmark to use `--bench-iterations 1` for first-hit measurement.

### 6. h8-v0-pinned (budget exhausted — no data)

**GPU memory pinning flow (context for this and next run):** In the normal lookup path, the client allocates GPU memory via PyTorch (`torch.zeros(..., device="cuda:0")`), obtains a CUDA IPC handle, and sends it to the server over gRPC. The server calls `cudaIpcOpenMemHandle` to access that GPU memory, then calls `prepare_memory_for_spdk()` to register the buffer with SPDK for BAR1/DMA visibility (via GDRCopy `nvidia-peermem` mapping). All pinning and SPDK registration happens server-side — the Python client only provides a destination GPU buffer address. The "cold pinning" overhead (5-9ms per request) comes from `prepare_memory_for_spdk()` being called on every lookup. The pre-pinned approach moves this to server init: the server allocates its own GPU staging buffer, registers it once with SPDK, and reuses it across all lookups — then does a device-to-device copy from staging to the client's buffer.

**What Nous designed:** The Opus designer autonomously read prior findings from `.nous/h8-v0-vs-p2p/` (1.33x slower due to cold pinning), identified the root cause, and designed an experiment with three arms: (A) existing bounce path as baseline, (B) P2P with persistent staging (pre-pinned at init), (C) P2P without DRAM promotion (ablation to isolate raw transfer time). Implementation: add `p2p_staging: Mutex<Option<Arc<DmaBuffer>>>` field, call `prepare_memory_for_spdk()` once during `initialize()`, reuse across all lookups.

**What happened:** Nous chose to add fields to the shared `IpcHandle` struct in `idispatcher.rs`, which cascaded to both dispatcher versions, all benchmarks, integration tests, and the connector — 12 files changed in a 597-line patch. The executor spent 240 turns (the maximum) trying to fix build errors from this cross-cutting interface change and never produced a running benchmark. Cost: $14.10 ($2.56 design + $11.54 executor).

**Failure mode:** Implementation approach too invasive. A local change (P2P path only in v0, no shared interface modifications) would have been achievable within budget, but Nous chose to thread new fields through the entire interface layer.

### 7. h8-v1-pinned

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

### 8. h8-evolve-v0-pipelined

Told to "overlap NVMe reads with GPU copies (true pipelining)" in dispatcher v0, without referencing v1's implementation. Most explicit direction given to any run.

**Iter-1 results (double-buffered async pipeline):**

| Arm | Condition | SSD-tier (μs) | Speedup vs baseline |
|-----|-----------|--------------|---------------------|
| h-main | Sequential v0 (baseline) | 19,502 | — |
| h-main | Pipelined v0 (treatment) | 9,659 | **2.02x** |
| h-control-negative | Sequential v0, 4 KiB | 1,749 | — |
| h-control-negative | Pipelined v0, 4 KiB | 1,466 | 1.19x |
| h-robustness | P2P cold (no pre-pinning) | 20,136 | 0.97x (slower) |

**Iter-1 analysis:**
- True pipelining achieves 2.02x over sequential v0 — matches pre-pinned P2P from h8-v1-pinned (9.3ms). The hypothesis is essentially confirmed: pipelined bounce ties P2P.
- The 2x gain is primarily from eliminating 32× per-chunk `DmaBuffer::new` allocations (~300μs each = ~9.6ms). The async overlap (GPU copy of chunk N during NVMe read of chunk N+1) is likely negligible — h8-v1-true-pipeline proved overlap saves <5% when allocation is already eliminated. However, we never tested v0 with *just* a buffer pool (no overlap), so this is an inference from the v1 result, not a direct v0 measurement.
- Control-negative (4 KiB, single chunk) shows 19% improvement even with no overlap opportunity — confirms buffer pre-allocation alone saves measurable overhead.
- A simpler fix for v0 would have been: pre-allocate a contiguous DmaBuffer at init, reuse across lookups — no double-buffering, no CUDA streams needed. The overlap machinery is unnecessary complexity solving a <5% problem.
- P2P cold (robustness arm) is 3% slower than sequential bounce — confirms cold pinning kills P2P without persistent staging.

**Iter-2 results (BatchSubmit QD=32 — different approach):**

| Arm | Condition | SSD-tier (μs) | Speedup vs baseline |
|-----|-----------|--------------|---------------------|
| h-main | Sequential v0 (baseline) | 27,746 | — |
| h-main | BatchSubmit QD=32 + async copy | 28,030 | 1.01x (no gain) |
| h-ablation | BatchSubmit QD=32 + sync copy | 20,236 | 1.37x |

**Iter-2 analysis:**
- Nous abandoned the working double-buffer approach and tried BatchSubmit QD=32 (all 32 reads in parallel). Zero improvement — gRPC round-trip + `connect_client()` channel setup (~15-25ms) completely dominates NVMe read time (~0.8ms at QD=32).
- High variance (±30-50%) across measurements makes sub-5% improvements undetectable.
- The iter-1 findings about `DmaBuffer::new` overhead were re-evaluated: iter-2 suggests the 2x was partly system state variance, not just buffer allocation. But iter-1 used `--bench-iterations 1` with fresh server restarts — warm-cache shouldn't apply.
- Failure mode #7: abandoned working approach without understanding why it worked.

**Did Nous answer the hypothesis?** Yes — within this run, pipelined bounce (9,659μs) is 2.08x faster than P2P (20,136μs). Nous correctly declared "CONFIRMED." The P2P implementation here uses cold pinning (per-request `prepare_memory_for_spdk`), which is what the campaign produced. Cross-run comparison against pre-pinned P2P (9,310μs from h8-v1-pinned) shows they're tied — but that's our observation, not a flaw in Nous's experiment design.

**Cost:** $15.59

### 9. h8-v1-true-pipeline

Told to "fix v1 to truly overlap reads and copies" — same directive as h8-evolve-v0-pipelined but targeting v1's existing ring-buffer architecture. Used 10 objects for better statistics (autonomous decision).

**Iter-1 results (cudaHostRegistered mmap'd buffers):**

| Arm | Condition | SSD-tier (μs) | vs baseline |
|-----|-----------|--------------|-------------|
| h-main | Sequential v1 (baseline) | 21,362 | — |
| h-main | Pipelined v1 (ReadAsync + cudaMemcpyAsync) | 19,220 | 10% faster |
| h-control-negative | P2P pre-pinned (cached prepare_memory_for_spdk) | 16,396 | 23% faster |

**Iter-1 analysis:**
- Only 10% gain from pipelining — `cudaMemcpyAsync` on `cudaHostRegistered` mmap'd memory falls back to synchronous (same finding as h8-pipelined). Correctly diagnosed; recommends `cudaHostAlloc` for iter-2.
- P2P with real pre-pinning (buffer cache) is 23% faster than baseline — consistent, reproducible advantage.
- All absolute latencies 3-5x higher than h8-v1-pinned historical results — system-level confound (NVMe thermal state, SPDK contention).

**Iter-2 results (cudaHostAlloc staging — the "correct" fix):**

| Arm | Condition | SSD-tier (μs) | vs baseline |
|-----|-----------|--------------|-------------|
| baseline | Sequential v1 (run 1) | 19,646 | — |
| baseline | Sequential v1 (run 2) | 23,717 | — |
| h-main | cudaHostAlloc pipeline (run 1) | 22,523 | 14.6% slower |
| h-main | cudaHostAlloc pipeline (run 2) | 21,412 | within noise |
| h-control-negative | P2P pre-pinned | 15,879 | 19% faster |

**Iter-2 analysis:**
- **cudaHostAlloc pipeline provides zero improvement over v1 baseline.** Three reasons: (1) async overlap is irrelevant — NVMe read (~600μs/chunk) dominates GPU copy (~5-50μs/chunk) by 10-100x; (2) the v1 patch allocates 2× `cudaHostAlloc` + `spdk_mem_register` **per call** (not pre-allocated at init like the v0 patch) — so it trades one per-call allocation cost for another, possibly more expensive one; (3) it also creates/destroys a CUDA stream per call.
- This explains why h8-evolve-v0-pipelined showed 2.02x while h8-v1-true-pipeline showed 0%: the v0 patch pre-allocates buffers once (lazy init in `Mutex<Option<PipelineState>>`, reused across all lookups), eliminating 32× `DmaBuffer::new`. The v1 patch didn't pre-allocate — it just swapped `DmaBuffer::new` for `cudaHostAlloc` per call, gaining nothing. If the v1 patch had pre-allocated its cudaHostAlloc buffers at init (like v0's patch does), it would likely show improvement from eliminating the 4× ring allocation.
- P2P pre-pinned remains consistently fastest (19-23% over baseline across both iterations).
- Nous correctly diagnosed the root cause: "NVMe read dominance is the binding constraint; GPU copy overlap is a second-order effect at 128 KiB chunk sizes." It also noted BatchSubmit QD=32 could parallelize reads — but didn't connect the dots that the pipeline's fundamental problem is sequential reads (one chunk at a time), not the lack of GPU copy overlap. The real fix isn't better pipelining, it's parallel NVMe submission — which the dispatcher's actor model doesn't easily support.

**Cost:** $19.92

---

## Conclusion: What Should Certus Use?

**Answer to the original hypothesis:** Refuted — but nuanced. Async overlap (hiding GPU copy behind NVMe read) saves <5% because NVMe read dominates by 10-100x at 128 KiB chunks. However, bounce with pre-allocated buffers (eliminating 32× `DmaBuffer::new`) reaches parity with pre-pinned P2P (~9.7ms vs ~9.3ms). Pre-pinned P2P is 19-23% faster than sequential bounce without buffer optimization, but against optimized bounce the advantage disappears. Cold P2P (per-request pinning) is slower than both.

**Did Nous answer correctly?** Mixed. h8-evolve-v0-pipelined correctly identified a 2x gain but misattributed it to overlap (it was buffer allocation elimination). h8-v1-true-pipeline correctly diagnosed the root cause in iter-2: NVMe read dominates, GPU copy overlap is irrelevant at this chunk size.

**What Certus should implement:**
- **Either path works with proper buffer management.** Pre-pinned P2P and bounce-with-pre-allocated-buffers achieve near-parity (~9.3ms vs ~9.7ms). P2P is simpler (one staging buffer, no per-chunk allocs) but requires GPU BAR1 visibility + SPDK registration. Bounce is more portable but needs a DmaBuffer pool to avoid the 32× allocation overhead that makes sequential bounce 2x slower.
- **Whichever path: pre-allocate buffers at connection time.** The dominant bounce improvement (2x) comes from eliminating per-lookup `DmaBuffer::new`, not from async overlap. P2P requires one-time `prepare_memory_for_spdk()` — without it, cold P2P is 1.3-2.7x slower than bounce.
- **Reduce system overhead first** — gRPC/connect_client/extent-manager overhead (~15-25ms) is the dominant cost. Both P2P (15.9ms) and bounce (19.6ms) are far from the isolated test binary results (2.3ms / 9.3ms). Fixing infrastructure yields larger gains than any DMA path optimization.

**What remains unproven:**
- Where the system overhead lives (all dispatcher measurements are 3-5x slower than historical h8-v1-pinned results — system confound or regression?)
- Whether the h8-evolve-v0-pipelined 2.02x result is reproducible or was a favorable system-state artifact
- Sustained multi-key throughput (only h8-v1-true-pipeline used 10 objects; others used 1)

**Practical impact:** Pre-pinned P2P saves ~4ms per 4 MiB lookup vs bounce through the dispatcher. For a 70B model (~140GB KV-cache, 35k lookups at 4 MiB), that's ~140s on cold-start restore. But system overhead (~15ms constant per lookup) is the larger target — reducing it to 2ms would save ~455s regardless of DMA path.

## Next Hypotheses

**Why pipelining was the wrong question:** The hypothesis assumed NVMe read and GPU copy are balanced enough that overlapping them matters. They're not — at 128 KiB sequential reads through the dispatcher, NVMe read is ~600μs/chunk while GPU copy is ~5-50μs/chunk (10-100x imbalance). The "pipeline" overlaps GPU copy of chunk N with NVMe read of chunk N+1, but the overlap window (~50-100μs of memcpy + GPU copy) barely dents the 600μs NVMe wait. What would actually help is issuing all 32 reads in parallel (QD=32 to the NVMe controller, collapsing 32×600μs to ~800μs total) — but the dispatcher's actor model issues reads sequentially, and when BatchSubmit was tried, gRPC overhead masked the NVMe parallelism anyway. The actual performance wins found were from buffer allocation elimination (2x) and P2P path selection (19-23%), not from overlap.

1. **System overhead profiling** — the dispatcher adds 15-25ms over the isolated test binary for the same operation. Where does it go? (gRPC serialization, `connect_client` channel setup, extent-manager lookup, DmaBuffer allocation, memory-tier promotion logic). This is 3-10x larger than any DMA path difference and affects both P2P and bounce equally.
2. **Buffer pool pre-allocation** — h8-evolve-v0-pipelined's 2x gain came from eliminating 32× `DmaBuffer::new` per lookup. Implement a per-connection buffer pool (pre-allocate N × 128 KiB DmaBuffers at connection time, reuse across lookups). Measure: how much of the 19ms is allocation vs actual I/O?
3. **Larger effective chunk sizes** — at 128 KiB, 32 sequential NVMe reads dominate latency. Can we coalesce into fewer, larger operations? (NVMe MDTS limits individual reads, but can we submit adjacent LBAs as one larger read if MDTS allows?)
4. **Direct actor-level benchmarking** — bypass gRPC entirely to measure true NVMe + DMA performance through the dispatcher's actor. This isolates whether the overhead is in gRPC/networking or in the dispatcher logic itself.
5. **Sustained multi-key throughput** — all experiments measured single-key (or 10-key) latency. Under sustained load, buffer reuse, NVMe queue depth, and GPU stream scheduling may behave differently.


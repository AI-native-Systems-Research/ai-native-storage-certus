# Hypothesis 8 — Nous Experiment Analysis

## Nous Experiment Structure

Each Nous experiment is a **bundle** with up to 4 **arms**:

| Arm | Purpose | What it answers |
|-----|---------|-----------------|
| **h-main** | Core hypothesis test | "Is the predicted effect real?" The primary A-vs-B comparison. |
| **h-control-negative** | Sanity check / known-bad baseline | "Does my measurement work?" Tests a condition with a known outcome (e.g., P2P-cold must be slow). If this fails, the whole experiment is suspect. |
| **h-ablation** | Remove one mechanism | "Is mechanism X the cause?" Disables one factor to see if the effect disappears. |
| **h-robustness** | Vary a parameter | "Does the effect generalize?" Tests under different conditions to rule out artifacts. |

---

## Original Hypothesis

> Using a bounce buffer SSD→CPU→GPU with pipelined transfers is faster than direct SSD→GPU for transfer of 4 MiB broken into a stream of 128 KiB transfers.

## What Nous Was Given

```yaml
research_question: >
  Using a bounce buffer SSD->CPU->GPU with pipelined transfers is faster
  than direct SSD->GPU for transfer of 4MiB broken into a stream of 128KiB
  transfers.
```

Minimal campaign — just the research question, target binary path, observable metrics, controllable knobs. No prior findings, no handoff, no hints.

## What Nous Did

Designed a 2-arm bundle with **no code changes**:

| Arm | What it tested | Prediction | Result |
|-----|---------------|------------|--------|
| **h-main** | Existing bounce vs P2P (pre-pinned) | Bounce faster | REFUTED — P2P 2x faster |
| **h-control-negative** | P2P-cold (per-request pin/map) | Must be slowest | CONFIRMED — 5.6x slower than P2P warm |

No h-ablation. No h-robustness. No patches.

## Results

| Mode | Throughput | Avg Latency |
|------|-----------|-------------|
| Bounce | 1510 MB/s | 2.65 ms |
| P2P warm | 3031 MB/s | 1.32 ms |
| P2P cold | 535 MB/s | 7.49 ms |

## What Nous Got Wrong

**It never tested the hypothesis.** The research question says "with pipelined transfers" — meaning concurrent SSD→RAM and RAM→GPU stages running in parallel. The existing `handle_bounce` code does them sequentially:

```
Phase 1: BatchSubmit 32 NVMe reads → wait for all completions
Phase 2: 32 sequential cudaMemcpy H2D copies
```

This is **not** pipelined. Pipelined means:

```
SSD→RAM: [chunk1] [chunk2] [chunk3] ...  (NVMe DMA, ongoing)
RAM→GPU:          [chunk1] [chunk2] ...  (cudaMemcpyAsync, concurrent)
```

Nous should have:
1. Read the code and noticed `handle_bounce` is two-phase sequential
2. Recognized this doesn't match "pipelined transfers" in the research question
3. Designed a code change implementing true pipelining (cudaMemcpyAsync + concurrent NVMe completions)
4. Tested pipelined bounce vs P2P

Instead it tested the existing non-pipelined bounce, found it slower than P2P, and declared the hypothesis refuted — answering a different question than what was asked.

## What Nous Found During Iter-1 Design (But Didn't Act On)

In the iter-1 handoff document, Nous explicitly noted:

> "Initially expected 'pipelining' to be the mechanism (overlapping NVMe reads and H2D copies in bounce mode). Reading the code revealed there is NO pipelining — both modes do read-all-then-copy-all sequentially."

And under "What I Excluded":

> "Pipelining optimization (overlap NVMe reads with copies): The current code does read-all-then-copy-all. A pipelined version would be a **code change experiment for iteration 2 if bounce wins**"

And under "Suggested next":

> "If bounce wins, iteration 2 should test a pipelined bounce variant"

So Nous correctly identified that pipelining doesn't exist in the code — but conditioned its implementation on "if bounce wins." When bounce lost, it pivoted to diagnostics instead of recognizing: the hypothesis was specifically *about* pipelining, the code doesn't have it, therefore the hypothesis was never tested.

It also noted the MDTS constraint (`128 KiB is stated as the NVMe MDTS limit`) which is actually a hardcoded default in our code (`controller.rs:158`), not necessarily the hardware limit. The entire 32×128 KiB framing depends on this assumption.

---

## Iteration 2: Latency Decomposition (Still No Pipelining)

Given iter-1's finding that P2P wins 2x, Nous designed iter-2 to understand *why* — decomposing latency into NVMe read phase vs copy phase.

**New research question (self-generated):**
> Is the 2x gap caused by the copy phase (H2D vs D2D) or the NVMe read phase (host-DMA vs GPU BAR1)?

**What it designed:** 3-arm bundle with code changes (instrumentation + `--skip-nvme` flag):

| Arm | What it tested | Prediction | Result |
|-----|---------------|------------|--------|
| **h-main** | Per-phase timing (read_us, copy_us) | Copy phase >1.5x slower in bounce | CONFIRMED — 7.2x slower (H2D 819μs vs D2D 114μs) |
| **h-ablation** | Skip NVMe reads entirely (`--skip-nvme`) | Copy-only ratio matches full-path ratio | CONFIRMED — 6.8x (no NVMe interference) |
| **h-control-negative** | Compare NVMe read time across modes | Within 20% of each other | CONFIRMED — 10% difference (790μs vs 710μs) |

**Results:**

| Mode | NVMe Read | Copy | Total (server-side) |
|------|-----------|------|---------------------|
| Bounce | 790 μs | 819 μs (H2D) | ~1610 μs |
| P2P warm | 710 μs | 114 μs (D2D) | ~824 μs |

**Key insight Nous found:** The copy phase is 7x slower in bounce (H2D at ~4.9 GB/s vs D2D at ~35 GB/s). NVMe read time is nearly identical regardless of DMA target. The 2x end-to-end gap is because NVMe read time (~750μs) dilutes the 7x copy difference.

**What this actually proves about pipelining:**

The iter-2 data *strongly supports* the pipelining hypothesis without Nous realizing it:
- Bounce spends 790μs on NVMe reads and 819μs on H2D copies — nearly equal
- These two phases use **independent hardware** (NVMe DMA engine vs GPU copy engine)
- With true pipelining: total time ≈ max(790, 819) = ~820μs instead of 790+819 = ~1610μs
- That would bring bounce to ~820μs — nearly matching P2P's ~824μs

Nous decomposed the latency perfectly and never noticed that the two phases it measured are exactly the kind that benefit from pipelining — because it already decided "bounce lost, investigate why" rather than "bounce wasn't tested properly, implement the hypothesis."

---

## Nous Assessment (Both Iterations)

- **Code discovery:** Excellent. Found all modes, identified GDRCopy overhead, noted MDTS constraint, discovered pipelining doesn't exist in the code
- **Experiment design:** Clean controls, reproducible, correct measurement protocol, good instrumentation in iter-2
- **Iter-2 science:** High quality latency decomposition — the phase breakdown data is exactly what we need
- **Critical failure:** Ignored "pipelined" in the research question across both iterations. Conditioned implementing it on "if bounce wins" — circular logic since bounce can't win without pipelining
- **Irony:** Iter-2's own data shows NVMe read (790μs) ≈ H2D copy (819μs) — the ideal scenario for pipelining. Nous measured the evidence for the hypothesis and didn't see it

---

## Follow-Up Runs: h8-evolve-v0 and h8-dispatcher-p2p

After the initial 2 iterations above, two additional Nous campaigns ran on 2026-05-18/19:

### h8-evolve-v0 — Pipelined Bounce (Channel Reuse)

**Goal:** Actually implement pipelined bounce and measure it against P2P warm.

**Iter-1 results:**

| Condition | Throughput | Avg Latency |
|-----------|-----------|-------------|
| Sequential bounce (baseline) | 1440 MB/s | 2.78 ms |
| Pipelined bounce (new) | 1764 MB/s | 2.27 ms |
| P2P warm (reference) | 3082 MB/s | 1.30 ms |

**Verdict: PARTIALLY_CONFIRMED (magnitude wrong).** Pipelining helped only 17% instead of the predicted ~50%. Root cause: the implementation called `connect_client()` per chunk (32 times), adding ~544μs of channel-creation overhead to the read phase. The async cudaMemcpy overlap IS working (copy dispatch drops from 826μs synchronous → 112μs async, confirming SPDK hugepages satisfy CUDA's pinned-memory requirement), but per-chunk channel allocation became the new bottleneck.

**Iter-2 (designed but not completed — stuck at EXECUTE_ANALYZE):** Rewrite to reuse a single channel across all chunks, eliminating the 544μs overhead. Predicted target: sub-900μs server-side latency.

**Principles extracted:**
1. SPDK hugepage DMA buffers satisfy CUDA's pinned-memory requirement — true non-blocking cudaMemcpyAsync confirmed
2. `connect_client()` costs ~13-17μs per call; 32 calls = 544μs wasted overhead
3. Pipelined bounce is fundamentally limited vs P2P by 2x PCIe bandwidth (NVMe→host + host→GPU vs NVMe→GPU direct)
4. Single CUDA stream is not a bottleneck (1-stream vs 2-stream: 0.6% difference)

**Note:** This run used the `gpu-p2p-server` benchmark harness, not the dispatcher's actual `pipeline.rs`. It replicated the dispatcher's pattern synthetically.

---

### h8-dispatcher-p2p — Path vs Submission Strategy Decomposition

**Goal:** Determine whether the dispatcher can benefit from P2P using its existing sequential ReadSync pattern (no BatchSubmit refactor needed).

**Status:** DONE (2 iterations completed, $10.41 cost)

**Iter-1:** Established baseline P2P-batch vs bounce-batch ratio (2.47x), P2P-cold penalty (2.74x slower), and 64 KiB chunk degradation.

**Iter-2 (key experiment):** Added sequential ReadSync variants of both P2P and bounce to isolate the path effect from the submission strategy effect.

| Condition | Avg Latency | Min Latency | Throughput |
|-----------|-------------|-------------|-----------|
| P2P sequential | 1.58 ms | 1.52 ms | 2534 MB/s |
| Bounce sequential | 2.32 ms | 2.27 ms | 1724 MB/s |
| P2P batch (reference) | 1.10 ms | 1.08 ms | 3641 MB/s |
| Bounce batch (reference) | 2.73 ms | 1.80 ms | 1465 MB/s |

**Verdicts:**
- h-main: **CONFIRMED** — P2P-seq is 1.47x lower latency than bounce-seq
- h-robustness: **CONFIRMED** — P2P-batch vs bounce-batch = 2.48x (reproduces iter-1)
- h-ablation: **PARTIALLY_CONFIRMED** — BatchSubmit improves bounce min latency by 1.26x, but *increases* avg latency due to 10-11ms tail spikes from NVMe queue saturation

**Surprise finding:** Bounce-batch avg (2.73ms) is WORSE than bounce-seq avg (2.32ms). BatchSubmit saturates the NVMe controller queue with 32 concurrent reads, causing occasional tail latency blowup on the bounce path. P2P-batch has no such problem (max-min spread <0.03ms), likely because BAR1-targeted DMA has more predictable completion ordering.

**Principles extracted:**
1. P2P with BatchSubmit = 2.5x over bounce (pre-pinned staging required)
2. Cold P2P (per-request pin) = 2.74x slower than bounce — pre-pinning is mandatory
3. 64 KiB chunks narrow P2P advantage from 2.47x → 2.04x (stay at 128 KiB MDTS)
4. **P2P path alone (sequential) = 1.47x** — sufficient for dispatcher integration
5. BatchSubmit causes tail amplification on bounce path; P2P is immune

**Note:** Also used `gpu-p2p-server`, not the dispatcher directly.

---

## Combined Assessment

### What the original hypothesis got wrong

The original hypothesis ("pipelined bounce is faster than direct P2P") is **refuted** even under the most favorable conditions:

- Best-case pipelined bounce (with channel reuse, iter-2 of h8-evolve-v0) is predicted at ~820μs server-side
- P2P warm with BatchSubmit achieves ~1.10ms end-to-end (including socket IPC)
- P2P sequential achieves ~1.58ms end-to-end

Even if pipelined bounce reaches its theoretical floor of max(read_us, copy_us) ≈ 820μs server-side, P2P-batch is already at ~650μs server-side. The fundamental limit is that bounce uses 2x PCIe bandwidth.

### What Nous actually proved (across all 4 iterations)

1. **P2P path advantage is real and sufficient for the dispatcher** — 1.47x with sequential submission (matching `pipeline.rs`) means P2P integration is justified without a BatchSubmit refactor
2. **Pre-pinned staging is mandatory** — cold P2P is 2.74x worse than bounce; the staging pool must persist across requests
3. **Sequential submission is safer for the bounce path** — BatchSubmit causes tail amplification that P2P is immune to
4. **Pipelining helps bounce but can't close the gap** — even perfect overlap gives ~820μs vs P2P's ~650μs

### Recommended action

Integrate P2P into the dispatcher's `pipeline.rs` using its existing sequential ReadSync pattern. Call `prepare_memory_for_spdk` via the `IGpuServices` receptacle to get BAR1-backed DMA buffers, then use them as read targets. Expected improvement: 1.47x latency reduction with minimal code change. No BatchSubmit refactor needed (and it would actually hurt the bounce fallback path via tail amplification).

### What remains untested

Neither run exercised the dispatcher end-to-end. All measurements are from the `gpu-p2p-server` benchmark harness. The 1.47x prediction for the dispatcher assumes:
- `prepare_memory_for_spdk` IPC overhead is negligible (amortized across the staging pool lifetime)
- The dispatcher's `connect_client()` + sequential ReadSync path behaves identically to the harness's replication of it
- No contention from the dispatcher's eviction/promotion logic during the transfer phase

A validation run through the actual dispatcher (`promote_and_serve` with P2P staging) is the logical next step.

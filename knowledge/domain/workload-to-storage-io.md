---
id: workload-to-storage-io
decision: How to map serving request patterns into storage IO operations and scheduling policies
answers:
  - How does serving behavior become storage IO?
  - What IO pattern does each inference phase generate?
  - How does the scheduler's batching strategy change IO shape?
  - What temporal patterns emerge from request arrival distributions?
  - When do IO bursts happen and what triggers them?
inputs:
  - request arrival rate and distribution (Poisson, bursty, periodic)
  - sequence length distribution (prompt + generation)
  - prefill-to-decode ratio in the running batch
  - scheduler policy (FCFS, shortest-job-first, preemptive, chunked)
  - batch size and continuous batching iteration cadence
  - prefix sharing ratio across concurrent requests
  - KV block size and per-token byte cost
  - tier latencies (GPU HBM, CPU DRAM, NVMe, network)
  - preemption rate and swap frequency
owns: ['request-to-IO mapping', 'scheduler effects', 'temporal patterns']
excludes:
  - block geometry and identity (see kv-footprint-and-lifecycle)
  - eviction policy selection (see tier-policy-and-eviction)
  - distributed placement and routing (see distributed-kv-ownership)
  - IO concurrency and pipeline mechanics (see data-movement-concurrency)
  - hardware specifications and device internals
related:
  - kv-footprint-and-lifecycle
  - tier-policy-and-eviction
  - data-movement-concurrency
  - cache-value-and-recompute
  - bottleneck-and-measurement
---

## 1. Decision Being Made

The optimization agent must decide:

1. **IO budget allocation**: Given a mix of prefill and decode requests, how much storage bandwidth should be reserved for each phase's IO pattern (bulk sequential writes vs. high-frequency small writes vs. large sequential reads)?
2. **Scheduling-aware prefetch**: Should the storage layer anticipate which blocks will be needed based on scheduler state (queue depth, priority ordering, preemption likelihood)?
3. **Temporal batching**: Should IO operations be issued immediately per-token/per-block, or accumulated and batched to exploit device-level parallelism?
4. **Burst absorption**: How much write buffering is needed to absorb the IO spikes that scheduling decisions create (e.g., a preemption event swapping out 256 blocks simultaneously)?
5. **Read/write ratio management**: Given that the same system simultaneously generates writes (new KV from prefill/decode) and reads (reloading shared prefixes, swap-in), how should the IO scheduler interleave them to avoid starvation?

These decisions determine whether storage keeps up with the serving engine or becomes the throughput bottleneck.

## 2. Mental Model and Equations

### Phase-to-IO Mapping

Each inference phase generates a distinct IO signature:

```
Cold prefill:     WRITE burst — ceil(prompt_tokens / block_size) blocks, sequential, bandwidth-bound
Warm prefill:     READ burst  — ceil(shared_prefix_tokens / block_size) loads, then WRITE for suffix
Decode:           WRITE trickle — 1 block per (block_size × token_latency) seconds, IOPS-bound
Preemption out:   WRITE burst — all active blocks for victim sequence, latency-critical
Swap-in:          READ burst  — all blocks for resumed sequence, latency-critical
Eviction:         DELETE batch — background, throughput-bound
```

### Aggregate IO Rate

```
write_rate_MBps = (prefill_tokens_per_sec + decode_tokens_per_sec) × kv_bytes_per_token / block_fill_ratio
```

Where:
- `prefill_tokens_per_sec`: total prefill throughput across all requests in batch
- `decode_tokens_per_sec`: batch_size × 1 token per iteration (for autoregressive)
- `kv_bytes_per_token`: 2 × num_kv_heads × head_dim × dtype_bytes × num_layers
- `block_fill_ratio`: fraction of block capacity used (accounts for partial sealing)

### Decode Write Cadence

```
decode_write_interval = block_size × decode_iteration_time
blocks_per_second_per_sequence = 1 / decode_write_interval
total_decode_IOPS = active_decode_sequences / decode_write_interval
```

For block_size=16, decode_iteration=30ms: one block sealed every 480ms per sequence.
With 256 concurrent decode sequences: 256/0.48 = 533 block writes/second.

### Prefill Burst Duration and IO Demand

```
prefill_burst_blocks = ceil(prompt_tokens / block_size)
prefill_burst_bytes = prefill_burst_blocks × block_bytes
prefill_burst_duration = prompt_tokens / prefill_token_throughput
required_write_bandwidth = prefill_burst_bytes / prefill_burst_duration
```

For 4096-token prompt, 16-token blocks, Llama-70B (5.12 MB/block): 256 blocks × 5.12 MB = 1.31 GB in ~200ms = 6.5 GB/s required write bandwidth.

### Continuous Batching IO Interleave

In each scheduler iteration:
```
iteration_writes = new_prefill_blocks_this_iter + newly_sealed_decode_blocks
iteration_reads = warm_prefix_loads_for_new_arrivals + swap_in_blocks
net_IO_per_iteration = (iteration_writes + iteration_reads) × block_bytes
IO_duty_cycle = net_IO_per_iteration / (iteration_time × device_bandwidth)
```

When IO_duty_cycle > 1.0, the storage device cannot keep up and becomes the bottleneck.

### Preemption IO Spike

```
swap_out_bytes = victim_sequence_blocks × block_bytes
swap_out_deadline = time_until_next_iteration (must complete before GPU needs the memory)
required_swap_bandwidth = swap_out_bytes / swap_out_deadline
```

For a 2048-token victim (128 blocks × 5.12 MB = 655 MB) with 30ms deadline: requires 21.8 GB/s — exceeding most NVMe device bandwidth, necessitating DRAM as swap tier.

### Temporal Request Patterns

```
arrival_burstiness = variance(inter_arrival_time) / mean(inter_arrival_time)²
cohort_size = count(requests arriving within prefix_load_window)
IO_amplification_factor = 1 / sharing_ratio  (lower is better; 1.0 = no sharing)
```

When `cohort_size > 1` and requests share a prefix, the storage system can coalesce reads:
```
actual_read_IO = unique_prefix_blocks + cohort_size × unique_suffix_blocks
naive_read_IO = cohort_size × (prefix_blocks + suffix_blocks)
coalescing_savings = 1 - (actual_read_IO / naive_read_IO)
```

## 3. Required Observations

Before deciding IO scheduling and buffering policy, the agent must measure:

| Observation | Why | How |
|-------------|-----|-----|
| Phase mix ratio (prefill vs decode tokens/sec) | Determines write pattern: bursty vs steady | Count tokens processed per phase per second from scheduler logs |
| Request arrival distribution | Sets burst amplitude and cohort overlap | Histogram inter-arrival times; fit Poisson/bursty model |
| Preemption frequency and victim size | Sizes swap buffer and burst write capacity | Count preemptions/minute; histogram victim sequence lengths |
| Prefix sharing ratio | Determines read coalescing opportunity | Measure unique_blocks_loaded / total_blocks_requested |
| Decode batch size | Sets sustained write IOPS floor | Read scheduler's active decode count |
| Chunked prefill chunk size | Determines prefill write granularity (many small bursts vs few large ones) | Read scheduler config; measure blocks sealed per chunk |
| Sequence length distribution | Predicts total blocks per request lifecycle | Sample from request logs (p50, p95, p99 prompt + generation) |
| Peak concurrent sequences | Upper bound on simultaneous IO streams | Monitor high-water mark of active sequences |
| Read-after-write latency | Whether newly written blocks are immediately readable (write-through vs writeback) | Time from store completion to successful load of same block |
| Device queue depth utilization | Whether IO scheduler is saturating device parallelism | Monitor NVMe submission queue occupancy |

## 4. Alternatives (with prefer_when / avoid_when)

### 4.1 IO Issuing Strategy

**Immediate per-block IO (write-through)**
- prefer_when: Low preemption risk (blocks rarely need urgent persistence), device bandwidth far exceeds steady-state demand, latency SLOs are loose, simplicity valued over throughput
- avoid_when: Decode generates many small blocks at high frequency (IOPS ceiling hit before bandwidth ceiling), device queue depth is shallow, write amplification from sub-optimal IO sizes is measurable

**Batched periodic flush (writeback with timer)**
- prefer_when: Decode phase dominates (many sequences generating 1 token at a time), device performs better with larger IOs, acceptable to lose unflushed blocks on crash
- avoid_when: Preemption is frequent (unflushed blocks lost on preempt), disaggregated architecture requires immediate visibility of blocks to remote decode node, strict durability requirements

**Event-driven flush (seal event triggers IO)**
- prefer_when: Mixed workload where prefill naturally seals full blocks (no buffering needed) and decode seals on timeout or phase boundary, want to minimize unnecessary IO while guaranteeing persistence at meaningful boundaries
- avoid_when: Seal events are too frequent (degenerates to write-through) or too rare (degenerates to large delayed batches that risk loss)

**Evidence:** Mooncake (FAST 2025) pipelines KV transfer with chunked prefill — each chunk's blocks are flushed to the distributed cache immediately upon chunk completion, overlapping IO with the next chunk's compute. This event-driven approach (seal on chunk boundary) achieved 525% throughput improvement by hiding transfer latency behind compute.

### 4.2 Read/Write Interleaving Policy

**Write-priority (reads queue behind writes)**
- prefer_when: New request throughput is the primary SLO, warm-prefix hits can tolerate 10-50ms additional latency, write bursts are short-lived and block reads only briefly
- avoid_when: TTFT is the binding SLO (prefix loads are on the critical path to first token), most requests are warm hits that need fast reads

**Read-priority (writes buffer in DRAM, reads go direct)**
- prefer_when: Most requests share cached prefixes (high read:write ratio), TTFT SLO is tight, DRAM write buffer is large enough to absorb prefill bursts without backpressure
- avoid_when: Write buffer is limited and prefill generates blocks faster than background flush can drain, crash would lose significant compute (unbounded write buffer = unbounded loss window)

**Fair-share with deadline awareness**
- prefer_when: Mixed workload with tight SLOs on both TTFT (reads) and throughput (writes), preemption swap-outs need deadline guarantees, neither phase can starve the other
- avoid_when: One phase dominates so heavily that fairness adds overhead without benefit (e.g., 95% decode-only workload where writes are the only IO)

**Evidence:** vLLM's swap mechanism (SOSP 2023) prioritizes swap-out writes over other IO during preemption — the GPU memory must be freed before the next iteration. This deadline-driven priority inversion demonstrates that IO scheduling cannot be static; it must respond to serving-layer urgency signals.

### 4.3 Prefetch Strategy for Warm Requests

**No prefetch (load on demand)**
- prefer_when: Prefix hit rate is unpredictable, queue depth is shallow (next request unknown until it arrives), storage latency is already low enough that on-demand reads don't violate TTFT SLO
- avoid_when: Requests are queued with visible prefixes, prefix load takes >50ms and TTFT SLO is <200ms, scheduler provides lookahead

**Queue-aware prefetch (load prefix for next N queued requests)**
- prefer_when: Scheduler queue is visible to storage layer, prefix identification is cheap (hash lookup), device has spare read bandwidth during decode-heavy periods, request ordering is stable (no priority inversions)
- avoid_when: Queue is volatile (requests reorder frequently), prefetch evicts useful cached data, device bandwidth is fully consumed by writes

**Speculative layer-selective prefetch**
- prefer_when: Not all layers' KV are needed simultaneously (pipelined layer execution), offloaded blocks are large (full model KV), and only a subset of layers are attention-bottlenecked in current step
- avoid_when: All layers execute simultaneously (no pipeline), blocks are small enough that full-block load is cheaper than selection overhead, prediction accuracy is low

**Evidence:** InfiniGen (OSDI 2024) predicts which KV blocks will have high attention scores using a lightweight prefetcher, loading only 10-30% of offloaded blocks per decode step while achieving up to 3× speedup. SGLang's RadixAttention enables O(1) prefix lookup via radix tree, making queue-aware prefetch nearly free — contributing to 6.4× throughput on multi-call workloads.

### 4.4 Temporal Batching Window

**Per-iteration batching (accumulate one scheduler iteration's IO)**
- prefer_when: Scheduler iteration time is predictable (20-50ms), device performs well with 10-100 IOs batched, natural alignment between compute and IO phases
- avoid_when: Iteration time varies widely (1ms to 500ms), some IOs are urgent (preemption) and cannot wait for batch

**Size-threshold batching (flush when accumulated bytes reach device-optimal IO size)**
- prefer_when: Device has strong preference for specific IO sizes (e.g., NVMe performs best at 128KB+ reads), write rate is variable but total bytes are predictable
- avoid_when: Latency-sensitive operations would be delayed waiting for threshold, threshold rarely reached during low-activity periods (stale data accumulates)

**Hybrid: urgent + batched (immediate for deadline IOs, batched for background)**
- prefer_when: Preemption and disaggregated handoff coexist with steady-state decode writeback, different IO classes have different latency requirements
- avoid_when: Complexity overhead exceeds benefit (single IO class dominates >95% of operations)

**Evidence:** FlexGen (ICML 2023) batches offload IO to match SSD optimal transfer size (512KB-2MB), computing an optimal offload schedule via linear programming. This enabled effective batch sizes of 144 on a single 16GB GPU by ensuring IO size always matched device sweet spots rather than issuing many small random writes.

### 4.5 Scheduler-Induced IO Shaping

**FCFS scheduling (first-come first-served)**
- prefer_when: Simple workload with uniform sequence lengths, no prefix sharing to exploit, fairness is the priority
- avoid_when: Prefix sharing exists (cohort scheduling can coalesce reads), variable-length requests cause head-of-line blocking that starves short requests and wastes swap IO on long-running sequences

**Prefix-grouped scheduling (batch requests sharing a prefix together)**
- prefer_when: High prefix commonality (same system prompt), shared prefix load can be amortized across cohort, TTFT SLO allows small queuing delay to form cohorts
- avoid_when: Prefix diversity is high (few requests share anything), grouping delay violates TTFT SLO, memory cannot hold the cohort simultaneously

**Preemption-aware scheduling (avoid preempting sequences near completion)**
- prefer_when: Preemption swap cost is high (large KV, slow storage tier), sequence progress is measurable, SLO allows slight unfairness to reduce total IO
- avoid_when: All sequences are similar length (no near-completion advantage), storage tier is fast enough that swap cost is negligible

**Evidence:** Mooncake's KVCache-centric scheduler explicitly balances throughput against SLO attainment, using prediction to reject requests early rather than admit-then-preempt. This reduced preemption-induced swap IO by eliminating the admit→preempt→swap→readmit cycle that wastes 2× the IO (swap-out + swap-in) for ultimately rejected requests.

## 5. Coupled Constraints

| This Decision | Interacts With | Mechanism |
|---------------|---------------|-----------|
| IO issuing strategy | Crash recovery | Writeback buffering means unflushed blocks are lost on crash; recovery must re-prefill |
| IO issuing strategy | Disaggregated handoff | Write-through is required for cross-node visibility; writeback adds transfer latency = flush delay |
| Prefetch policy | Eviction policy | Prefetched blocks consume cache capacity; aggressive prefetch may evict blocks needed by active sequences |
| Prefetch policy | Queue ordering | Prefetch value depends on scheduler not reordering; priority inversion wastes prefetched IO |
| Temporal batching | TTFT SLO | Larger batching window = lower device efficiency overhead but higher tail latency for first read |
| Scheduler grouping | Memory capacity | Cohort scheduling requires holding all cohort members' KV simultaneously; cohort_size × seq_len × kv_per_token must fit |
| Read/write priority | Preemption deadline | Swap-out must complete within one iteration (~30ms); if reads have priority, swap may miss deadline |
| Decode write cadence | Device wear (NVMe) | 533 writes/sec sustained = significant write amplification if blocks < device page size; batching reduces P/E cycles |
| Burst absorption buffer | Total memory budget | Write buffer memory competes with KV cache capacity; 1GB buffer = 1GB fewer active sequences |
| Prefetch depth | Network bandwidth (disaggregated) | Prefetching from remote nodes consumes same links used for active handoff; must not starve critical transfers |

## 6. Failure Modes

### IO Starvation During Phase Transitions

**Problem:** Continuous batching admits a burst of new prefill requests, generating a write spike that saturates the storage device. Concurrent decode sequences cannot flush their sealed blocks; GPU memory fills with unsealed blocks, triggering preemption, which demands MORE urgent writes.

- Symptom: Preemption rate spikes correlate with new-request admission bursts
- Detection: Monitor IO queue depth at admission events; correlate preemption rate with write queue saturation
- Root cause: No admission control coordinating request arrival rate with storage write capacity

### Prefetch Pollution

**Problem:** Queue-aware prefetch loads prefix blocks for queued requests, but scheduler reorders the queue (priority arrival). Prefetched blocks evict currently-needed blocks from fast tier. Active sequences stall waiting for re-fetched blocks.

- Symptom: Cache hit rate drops during queue reordering events; "useful eviction" counter spikes
- Detection: Track prefetch-then-evict-before-use ratio; if >20% of prefetched blocks are evicted unused, prefetch is net-negative
- Root cause: Prefetch assumes stable ordering; dynamic priority violates this assumption

### Decode IOPS Ceiling

**Problem:** 512 concurrent decode sequences, 16-token blocks, 30ms iteration → 533 block writes/sec. Each block is 5.12 MB. If issued individually: 533 random writes/sec × 5.12 MB = 2.7 GB/s random write bandwidth. NVMe device handles 3.5 GB/s sequential but only 1.2 GB/s random — system hits random-write IOPS ceiling at ~235 concurrent sequences, well below the 512 target.

- Symptom: Decode throughput plateaus as batch size increases; device utilization at 100% but achieved bandwidth far below sequential peak
- Detection: Compare achieved MB/s to device sequential-write spec; ratio < 0.5 indicates random-IO penalty
- Root cause: Per-sequence writes land at scattered addresses; no spatial locality between sequences

### Swap Deadline Miss

**Problem:** Preemption victim has 2048 tokens (128 blocks × 5.12 MB = 655 MB). Swap-out to NVMe must complete before next scheduler iteration (30ms). Required bandwidth: 21.8 GB/s — exceeds device capability. Swap does not complete; GPU memory not freed; scheduler stalls.

- Symptom: Scheduler iteration time exceeds target during preemption; "swap incomplete" warnings
- Detection: Measure swap-out duration vs iteration deadline; histogram of deadline violations
- Root cause: Swap tier cannot absorb full sequence in one iteration; need DRAM swap buffer or partial-swap strategy

### Thundering Herd on Shared Prefix Load

**Problem:** 64 requests arrive in 100ms window, all needing same 1024-block system prompt prefix. Without coalescing, storage sees 64 × 1024 = 65,536 read requests for the same blocks. Even with dedup at block level, 64 concurrent load paths contend for same buffer memory.

- Symptom: Massive read amplification despite high prefix sharing; memory allocator contention spikes
- Detection: Monitor unique_blocks_loaded vs total_block_load_requests; ratio << 1 indicates missing coalescing
- Root cause: No coalescing layer between request admission and block read path

### Write Buffer Exhaustion Under Backpressure

**Problem:** Writeback buffer absorbs decode writes during device saturation. But prolonged saturation (sustained prefill burst) fills the buffer. Once full, decode write path blocks synchronously, adding latency to the decode iteration, increasing TPOT.

- Symptom: TPOT spikes correlate with write buffer high-water events; decode latency becomes bimodal
- Detection: Monitor write buffer occupancy; correlate high-water events with TPOT p99 spikes
- Root cause: Buffer sized for transient bursts, not sustained overload; need backpressure signal to admission control

## 7. Hypotheses the Agent Can Generate

1. **H: Switching from per-block write-through to seal-event batched flush will reduce device IOPS by >4× during decode-heavy phases without increasing block loss risk.**
   - Basis: 256 decode sequences with block_size=16 produce one block every 480ms. Batching 4 consecutive seals into one 4-block write: IOPS drops from 533 to 133 while IO size increases from 5.12 MB to 20.5 MB (better device utilization). Loss window = 4 × 480ms = 1.92s per sequence.

2. **H: Prefix-grouped scheduling will reduce total read IO by >50% for this workload's measured sharing ratio of 0.6.**
   - Basis: With sharing_ratio=0.6, each request's prefix overlaps 60% with others. Cohort of 8 simultaneous requests: naive reads = 8P blocks; coalesced reads = P + 8×0.4P = 4.2P blocks. Savings = 1 - 4.2/8 = 47.5%.

3. **H: Adding a 2GB DRAM write buffer will eliminate swap deadline misses for sequences up to 4096 tokens.**
   - Basis: 4096 tokens ÷ 16 block_size × 5.12 MB = 1.31 GB. DRAM bandwidth (~100 GB/s) completes this in 13ms — well within 30ms deadline. Current NVMe path requires 655ms for same sequence.

4. **H: The current IO pattern is random-write-bound, not bandwidth-bound. Reordering writes by block address will increase effective throughput by >2×.**
   - Basis: If blocks from different sequences land at random offsets, device sees random-write pattern (1.2 GB/s effective). Sorting pending writes by physical address converts random to sequential (3.5 GB/s effective).

5. **H: Queue-aware prefetch with depth=3 will reduce average TTFT by >30% given the observed 85ms median prefix load time and 200ms TTFT SLO budget.**
   - Basis: If prefix load overlaps with queue wait time (avg 120ms in queue), and load completes in 85ms, 3-deep prefetch ensures most requests have prefix ready by scheduling time. TTFT reduction = min(85ms, time_in_queue).

6. **H: Chunked prefill with chunk_size=512 tokens generates smoother write traffic than full-prompt prefill, reducing peak write bandwidth requirement by >3×.**
   - Basis: Full 4096-token prefill generates 256 blocks in ~200ms = 6.5 GB/s peak. Chunked into 8 chunks: each generates 32 blocks interleaved with decode IO and compute. Peak per-chunk = 32 × 5.12 MB / 25ms = 6.5 GB/s per chunk but with 25ms gaps for compute → average only 2.1 GB/s sustained.

## 8. Experiments and Falsifiers

### E1: IO Pattern Characterization Under Continuous Batching

**Tests H4 (random vs sequential).** Capture block-level IO trace during steady-state continuous batching.
- Measure: IO address entropy, sequential-access ratio, device utilization at measured IOPS vs peak sequential spec
- Falsifier for H4: If sequential-access ratio > 0.6 already (blocks naturally cluster by sequence), reordering adds <10% improvement and hypothesis is wrong
- Method: Trace all block store/load addresses; compute autocorrelation of address stream

### E2: Batched vs Immediate Flush During Decode

**Tests H1.** Compare per-seal-event write-through against N-block batched writeback.
- Measure: Device IOPS, effective bandwidth, decode iteration jitter, block loss count under simulated crash
- Falsifier for H1: If device handles per-block IOPS without saturation (queue depth < 50% capacity), batching adds complexity without benefit. Or if crash-induced block loss is unacceptable even with 2s window.
- Control: Same decode batch, same sequence lengths; vary only flush policy

### E3: Cohort Scheduling Read Reduction

**Tests H2.** Compare FCFS scheduling against prefix-grouped cohort scheduling.
- Measure: Total block reads issued, unique block reads, TTFT distribution, memory high-water mark
- Falsifier for H2: If cohort formation delay > 50ms (waiting for enough shared-prefix requests) and TTFT SLO is 200ms, the queuing cost exceeds the IO savings. Or if sharing_ratio < 0.3, coalescing saves <25%.
- Control: Replay same request trace with both schedulers; measure IO and TTFT

### E4: DRAM Swap Buffer Sizing

**Tests H3.** Vary swap buffer size ∈ {512MB, 1GB, 2GB, 4GB} under workload with measured preemption rate.
- Measure: Swap deadline miss rate, swap-out latency p99, impact on available KV cache capacity
- Falsifier for H3: If preemptions are rare (<1/minute) or victim sequences are short (<512 tokens), even 512MB buffer has 0% deadline misses, and 2GB wastes capacity that could hold more active KV
- Control: Same workload trace with injected preemptions at measured rate

### E5: Prefetch Depth vs TTFT

**Tests H5.** Vary prefetch depth ∈ {0, 1, 3, 5, 10} queued requests.
- Measure: TTFT p50/p95/p99, prefetch hit rate (prefetched blocks used before eviction), cache pollution rate
- Falsifier for H5: If queue ordering is unstable (>30% of prefetched requests are reordered away from front), prefetch hit rate drops below 50% and TTFT improvement < 10%
- Method: Instrument queue reorder events; compute prefetch utility as function of depth and queue stability

### E6: Chunked Prefill Write Smoothing

**Tests H6.** Compare unchunked (full prompt in one prefill) vs chunked prefill (512-token chunks).
- Measure: Peak write bandwidth, write bandwidth variance (coefficient of variation), impact on concurrent decode latency
- Falsifier for H6: If the device can sustain 6.5 GB/s peak writes without affecting decode (separate queues or sufficient depth), chunking adds scheduling complexity without measurable benefit
- Control: Same prompts; vary only chunk_size parameter in scheduler

### E7: Write Reordering Throughput Gain

**Tests H4 directly.** Implement write reorder buffer that sorts pending block writes by storage address.
- Measure: Device-reported sequential write fraction, achieved MB/s, reorder buffer latency overhead
- Falsifier for H4: If reorder buffer must hold >100ms of writes to achieve >70% sequential fraction (too much latency added), or if device controller already handles random patterns efficiently (SSD FTL makes physical layout opaque), reordering at software level provides <20% gain
- Method: A/B test with and without reorder buffer; measure at device level, not application level

## 9. Production Evidence

### Mooncake KVCache-Centric Architecture
- **System:** Mooncake (Qin et al., FAST 2025), Kimi production serving
- **Problem:** Disaggregated prefill-decode requires predictable IO to transfer KV between nodes without violating TTFT SLOs under variable load
- **Mechanism:** KVCache-centric scheduler co-optimizes request admission with available cache/IO bandwidth. Chunked prefill pipelines KV writes with computation — each chunk's sealed blocks transfer to decode node while next chunk computes. Prediction-based early rejection eliminates admit→preempt→swap→readmit IO waste cycle.
- **Result:** 525% throughput improvement over baseline; 75% more requests handled in production (Kimi). Transfer latency hidden behind compute by pipelining.
- **Lesson:** The scheduler IS the IO policy. When the scheduler predicts it cannot service a request without preemption, rejecting early eliminates 2× redundant swap IO. Chunked transfer converts one massive burst into a pipelined stream the storage system can absorb incrementally.

### vLLM Preemption and Swap
- **System:** vLLM (Kwon et al., SOSP 2023)
- **Problem:** When GPU memory is exhausted by KV, sequences must be preempted. Naive preemption discards KV (recompute on resume) or swaps to CPU/disk (IO cost).
- **Mechanism:** PagedAttention enables block-granular swap — only swap specific blocks, not entire contiguous buffers. Swap-out writes blocks to CPU memory; swap-in reads them back. Non-contiguous physical layout means swap doesn't require gathering scattered data into contiguous buffers first.
- **Result:** 2-4× throughput via efficient memory use. Swap path adds block_count × per_block_transfer_time latency. For moderate sequences (512 tokens, 32 blocks), swap completes in ~5ms to CPU DRAM.
- **Lesson:** Block-granular swap converts a monolithic O(seq_len) copy into parallelizable per-block transfers. The IO pattern of preemption is inherently bursty and latency-critical — it must complete within one scheduler iteration or the pipeline stalls.

### DistServe Disaggregated Serving
- **System:** DistServe (Zhong et al., OSDI 2024)
- **Problem:** Co-located prefill and decode interfere — prefill's compute bursts delay decode iterations, inflating TPOT; decode's memory pressure forces premature preemption of prefill.
- **Mechanism:** Separate prefill and decode onto different GPU pools. KV transfer after prefill completes becomes a network IO operation. The system co-optimizes parallelism strategy per phase and minimizes inter-phase communication cost through intelligent placement.
- **Result:** 7.4× more requests served within SLO, or 12.6× tighter SLO achievable at same throughput. KV transfer cost is acceptable because prefill-decode interference cost was much larger.
- **Lesson:** Disaggregation converts a compute-scheduling problem into an IO-scheduling problem. The KV transfer between nodes is a new IO stream that didn't exist in co-located systems — it has strict deadline semantics (decode cannot start until transfer completes) and competes with other network traffic.

### SGLang RadixAttention Prefix Sharing
- **System:** SGLang (Zheng et al., 2024)
- **Problem:** LLM programs make multiple calls sharing large prefixes (system prompt + conversation history). Each call recomputes shared prefix KV from scratch.
- **Mechanism:** Radix tree indexes all cached KV blocks by token content. New requests traverse the tree to find longest cached prefix, load those blocks (READ), then compute only the suffix (much shorter WRITE). LRU eviction operates at block granularity within the tree.
- **Result:** 6.4× throughput on multi-call workloads (tree-of-thought), 5× on multi-turn chat. Effectively converts the IO pattern from "write entire sequence" to "read shared prefix + write short suffix."
- **Lesson:** Prefix sharing fundamentally changes the IO ratio. Without sharing: nearly 100% writes (all KV is new). With sharing at 80% hit rate: 80% reads + 20% writes. The storage system must handle this read-dominant pattern efficiently — the bottleneck shifts from write bandwidth to read latency (affects TTFT).

### FlexGen Tiered Offload
- **System:** FlexGen (Sheng et al., ICML 2023)
- **Problem:** GPU memory too small for large-batch inference. KV must spill to CPU and SSD.
- **Mechanism:** Linear programming computes optimal percentage of each layer's KV on each tier. Batch-level scheduling ensures all tokens in a batch access the same layer simultaneously, converting random per-token access into sequential per-layer sweeps. IO aligned to 512KB-2MB device-optimal transfer units.
- **Result:** Enabled batch size 144 on single 16GB GPU (vs 1-4 without offload); throughput-optimal when IO perfectly aligned to device characteristics. 1 token/s generation for OPT-175B on single GPU.
- **Lesson:** The scheduling policy determines the IO pattern. FlexGen's layer-by-layer batch execution converts what would be random per-token IO into sequential bulk IO. The same total bytes transferred, but 10-50× more efficiently because the access pattern matches device physics. This proves scheduling decisions dominate raw device capability.

### CacheGen Compressed Transfer
- **System:** CacheGen (Liu et al., SIGCOMM 2024)
- **Problem:** Transferring raw KV cache over network for prefix reuse is bandwidth-bound. 4096 tokens on Llama-70B = 1.31 GB per transfer.
- **Mechanism:** Custom tensor encoder compresses KV blocks using distributional properties (delta coding, adaptive quantization). Compression level adapts to available bandwidth — higher compression when link is congested, lower when bandwidth is plentiful.
- **Result:** 3.5-4.3× reduction in KV size; 3.2-3.7× reduction in total fetch+decode latency with negligible quality loss (<0.2 perplexity increase).
- **Lesson:** IO volume is not fixed by the workload — it can be traded against compute (decompression cost). When bandwidth is the bottleneck, compression converts a bandwidth-bound IO pattern into a compute-bound decompression pattern. The optimal compression level depends on the instantaneous bandwidth-to-compute ratio, making it a dynamic scheduling decision.

### InfiniGen Selective Retrieval
- **System:** InfiniGen (Lee et al., OSDI 2024)
- **Problem:** Long-context offloaded KV is too large to reload entirely per decode step. Loading all blocks wastes bandwidth on tokens that won't receive significant attention weight.
- **Mechanism:** Lightweight speculative prefetcher uses current-layer inputs and next-layer query/key weights to predict which KV entries will have high attention scores. Only predicted-essential blocks are fetched from host memory.
- **Result:** Up to 3× speedup over prior offload methods while loading only essential blocks (estimates suggest 10-30% of total offloaded KV per step, depending on attention sparsity pattern).
- **Lesson:** Not all stored blocks generate equal read IO in practice. Attention sparsity means most blocks are irrelevant to any given decode step. A prediction-guided IO scheduler that knows WHICH blocks matter can reduce read bandwidth by 3-10× compared to naive full-reload. This transforms the read pattern from "load everything" to "load predicted-hot subset" — a fundamentally different IO shape that storage must support efficiently (random reads of subset vs sequential scan of all).

### Xinnor MLPerf Storage Eviction Study
- **System:** MLPerf Storage 3.0 benchmarks with Xinnor eviction analysis
- **Problem:** Different eviction policies create different IO patterns on the storage device as blocks cycle through cache.
- **Mechanism:** Compared LRU, LFU, ARC, and GDSF (size-aware) policies. Each creates distinct temporal IO signatures: LRU produces sequential eviction bursts at cache-full events; LFU produces scattered individual evictions; ARC adapts between patterns.
- **Result:** Eviction policy changed achieved throughput by up to 40% on same hardware, purely through IO pattern effects. ARC (Megiddo & Modha, FAST 2003) performed best under mixed workloads by learning the workload's recency/frequency balance.
- **Lesson:** The eviction policy IS an IO scheduling policy. It determines when writes occur (eviction timing), which blocks are written (victim selection affects future read misses), and the spatial pattern of those writes. Optimizing eviction for "cache hit rate" without considering the IO pattern it creates misses significant throughput on real devices.

## Implications for KV Block Storage

1. **The storage system sees at least 5 distinct IO patterns simultaneously** (prefill writes, decode trickle writes, prefix reads, swap bursts, background eviction/GC) — each with different latency requirements, sizes, and arrival patterns. A single IO scheduler treating all operations equally will be suboptimal by 2-5×.

2. **The scheduler is the IO policy.** Every scheduling decision (admission, batching, preemption, prefix grouping) directly determines the IO pattern the storage layer must handle. Storage cannot be designed in isolation from the scheduler — it must receive scheduling signals (upcoming preemption, cohort formation, admission rate) to prepare.

3. **Read/write ratio is workload-determined and dynamic.** Fresh deployment with no cached prefixes: ~100% writes. Steady-state with high prefix sharing: 70-90% reads. The storage system must handle both extremes and the transitions between them without manual tuning.

4. **Preemption creates IO emergencies.** Swap-out has a hard deadline (one scheduler iteration, ~30ms). At scale (2048-token sequences), this requires burst bandwidth exceeding NVMe capability, making DRAM swap buffers mandatory. The storage system must distinguish deadline IO from background IO.

5. **Compression and selective retrieval make IO volume a tunable parameter.** CacheGen's 3.5-4.3× compression and InfiniGen's 70-90% read reduction show that raw byte volume is not determined solely by the workload — it's a function of how much compute you're willing to trade for bandwidth reduction. The storage system should expose bandwidth vs. compute tradeoff knobs.

6. **Cohort effects create natural coalescing opportunities.** When N requests share a prefix, naive implementation issues N identical read streams. Coalescing at the storage layer (single read, multicast result) reduces read amplification by N×. This is not optional at scale — 64-request cohorts without coalescing generate 64× the read IO.

7. **IO pattern stability depends on workload stationarity.** Chat workloads shift between exploration (diverse prefixes, write-heavy) and exploitation (repeated prefixes, read-heavy) over hours. Batch processing is more stable. The storage IO scheduler should detect regime shifts (see bottleneck-and-measurement) and adapt within seconds, not minutes.

8. **Write ordering matters more than write speed for NVMe tiers.** Random 5MB writes at 533/sec achieves ~1.2 GB/s on typical NVMe. Sequential writes of the same total bytes achieve 3.5+ GB/s. A reorder buffer that collects writes and issues them address-sorted can nearly triple effective write throughput — a free optimization if the latency budget allows 10-50ms reorder window.

I now have sufficient context on the format, scope, and evidence. Here's the full domain reference file:

---

```markdown
---
id: data-movement-concurrency
decision: How many parallel IO operations to issue and how to structure the pipeline stages that move KV blocks between tiers
answers:
  - How much useful IO parallelism exists?
  - What is the effective concurrency for a given transfer path?
  - How should pipeline stages be structured (write-behind, read-ahead, compress-then-send)?
  - Who owns each buffer in the transfer chain?
  - When should backpressure throttle the producer?
inputs:
  - per-tier bandwidth ceiling (HBM, PCIe, NVMe, RDMA, TCP)
  - per-operation latency (page fault, DMA setup, network RTT)
  - block size in bytes
  - available buffer pool memory per tier
  - number of concurrent requests generating IO demand
  - request criticality (SLO headroom remaining)
  - compression ratio achievable on KV tensors
  - device queue depth limits (NVMe submission queues, RDMA QPs)
owns: ['effective concurrency', 'pipeline stages', 'buffer ownership', 'backpressure']
excludes:
  - which blocks to move (see tier-policy-and-eviction)
  - block value computation (see cache-value-and-recompute)
  - distributed routing and placement (see distributed-kv-ownership)
  - hardware specifications and device internals
  - attention kernel internals
related:
  - tier-policy-and-eviction
  - workload-to-storage-io
  - bottleneck-and-measurement
  - kv-footprint-and-lifecycle
  - distributed-kv-ownership
  - correctness-and-recovery
---

## 1. Decision Being Made

The optimization agent must decide:

1. **Queue depth per path**: How many IO operations to keep in-flight simultaneously on each transfer path (GPU→host DMA, host→NVMe, host→remote via RDMA/TCP). Too few starves the device; too many wastes buffer memory and increases tail latency.
2. **Pipeline topology**: Whether to use a linear pipeline (stage A completes before stage B starts), overlapped pipeline (stage B begins on chunk N while stage A processes chunk N+1), or scatter-gather (fan-out to multiple targets concurrently).
3. **Buffer allocation and ownership**: How many buffers to pre-allocate at each stage boundary, what size they should be, and whether ownership transfers (zero-copy) or copies are used at each handoff.
4. **Backpressure policy**: When a downstream stage cannot consume fast enough, should the upstream stage block, drop, spill to a slower tier, or signal the scheduler to reduce admission?
5. **Compression placement**: Whether to compress before transfer (reducing bytes on the wire but adding CPU/GPU cycles) or transfer raw (saturating bandwidth but with simpler pipeline).

Getting these wrong either leaves expensive bandwidth idle or creates head-of-line blocking that stalls latency-critical decode requests behind bulk prefill transfers.

## 2. Mental Model and Equations

### Little's Law Applied to IO Pipelines

```
concurrency_needed = throughput_target / single_op_throughput
                   = throughput_target × latency_per_op / op_size
```

Or equivalently:
```
L = λ × W
```
Where L = in-flight operations, λ = arrival rate (ops/s), W = per-op service time (s).

**Example**: To sustain 50 GB/s over PCIe Gen5 x16 with 5 MB blocks and 10 μs DMA setup latency:
```
ops_per_sec = 50 GB/s ÷ 5 MB = 10,000 ops/s
concurrency = 10,000 × 10 μs = 0.1  (latency-bound: 1 op suffices for DMA setup)
```
But with NVMe at 3.5 GB/s, 5 MB blocks, 20 μs submission latency:
```
ops_per_sec = 3,500 MB/s ÷ 5 MB = 700 ops/s
concurrency = 700 × 20 μs = 0.014  (still <1, but real NVMe needs queue depth 8-32 for full bandwidth due to internal parallelism)
```

### Effective Bandwidth Under Pipeline Overlap

```
effective_bw = min(bw_stage_1, bw_stage_2, ..., bw_stage_N) × overlap_efficiency
```

For a two-stage pipeline (compress → transfer):
```
overlap_efficiency = 1 - max(0, (t_compress - t_transfer)) / (t_compress + t_transfer)
```
When compression and transfer take equal time, overlap_efficiency ≈ 1.0 (perfect overlap). When one dominates, efficiency degrades toward 0.5.

### Buffer Memory Budget

```
buffer_pool_bytes = num_stages × queue_depth_per_stage × buffer_size
```

Each buffer is owned by exactly one stage at any moment. The total budget constrains the maximum achievable concurrency:
```
max_concurrency = buffer_pool_bytes / (block_bytes × pipeline_depth)
```

### Backpressure Threshold

```
backpressure_trigger = queue_occupancy > high_watermark × queue_capacity
resume_trigger = queue_occupancy < low_watermark × queue_capacity
```

The gap between high and low watermarks (hysteresis) prevents oscillation. Typical values: high=0.8, low=0.5.

### Bandwidth-Delay Product for Network Transfers

```
BDP = bandwidth × RTT
buffers_needed = ceil(BDP / block_size)
```

For RDMA at 100 Gbps with 5 μs RTT: BDP = 62.5 KB (< one 5 MB block → 1 buffer suffices).
For TCP at 25 Gbps with 200 μs RTT: BDP = 625 KB (still < one block, but TCP windowing needs multiple in-flight for throughput).

### Compression-vs-Transfer Tradeoff

```
total_time_compressed = t_compress + (block_bytes × compression_ratio) / bandwidth
total_time_raw = block_bytes / bandwidth
```

Compression wins when:
```
t_compress < block_bytes × (1 - compression_ratio) / bandwidth
```

CacheGen achieves 3.5-4.3× compression (ratio 0.23-0.29), making compression almost always profitable over network links below ~50 Gbps (SIGCOMM 2024).

## 3. Required Observations

Before deciding concurrency and pipeline structure, the agent must measure:

| Observation | Why | How |
|-------------|-----|-----|
| Single-op latency per path | Determines minimum concurrency via Little's Law | Issue one op, measure end-to-end time including setup |
| Bandwidth saturation curve | Find the queue depth at which bandwidth plateaus | Sweep queue depth 1→64, measure throughput |
| Buffer memory available | Caps maximum achievable concurrency | Query allocator: total pool minus reserved for compute |
| Block size distribution | Affects op count and per-op efficiency | Sample from active workload |
| Compression ratio on live KV | Determines whether compress-before-send pays off | Encode sample blocks, measure ratio and encode time |
| Request SLO headroom | Distinguishes latency-critical from throughput-optimal paths | TTFT_budget - current_prefill_time |
| Device queue depth limits | Hard cap on in-flight ops per device | NVMe: spec'd per-SQ limit; RDMA: QP depth; GPU DMA: stream count |
| Concurrent request count | Total IO demand competing for shared paths | Count active sequences × their IO rate |
| Tail latency under load | Detect head-of-line blocking | Measure p99 at various concurrency levels |
| Producer-consumer rate mismatch | Identifies where backpressure is needed | Compare throughput at each stage boundary |

## 4. Alternatives (with prefer_when / avoid_when)

### 4.1 Queue Depth Strategy

**Shallow queues (1-4 ops in-flight)**
- prefer_when: Latency-critical decode path where each block fetch is on the SLO critical path; buffer memory is scarce; device already bandwidth-saturated at low depth (e.g., PCIe DMA)
- avoid_when: Device requires depth for internal parallelism (NVMe, network); throughput-oriented bulk transfers; high per-op setup cost amortizable over batch

**Deep queues (16-64 ops in-flight)**
- prefer_when: NVMe or network devices with internal parallelism; bulk writeback/demotion not on critical path; large buffer pool available; many small blocks where per-op overhead dominates
- avoid_when: Buffer pool is shared with compute and deep queues risk OOM; tail latency matters (deep queues increase variance); device has no internal parallelism to exploit

**Adaptive depth (start shallow, deepen under load)**
- prefer_when: Mixed workloads with both latency-critical and throughput-optimal transfers sharing the same path; load varies significantly over time
- avoid_when: Adaptation logic adds complexity; steady-state workloads where a fixed depth is optimal; very short transfer bursts where adaptation can't converge

**Evidence:** FlexGen's linear programming solver selects batch sizes that implicitly set queue depth to maximize overlap between GPU compute and CPU↔SSD transfers, achieving 1 token/s throughput on OPT-175B from a single 16GB GPU (ICML 2023). This required effective batch size 144—each batch element keeps the pipeline full.

### 4.2 Pipeline Topology

**Linear pipeline (stage-by-stage, no overlap)**
- prefer_when: Debugging/correctness verification; single-block transfers where overlap is impossible; stages share the same physical resource (same memory bus)
- avoid_when: Multi-block transfers where stages operate on different blocks concurrently; latency budget is tight; hardware supports DMA engines that run independently of CPU

**Overlapped chunked pipeline (double/triple buffering)**
- prefer_when: Multi-block bulk transfers (prefill handoff, writeback batch); stages operate on independent hardware (GPU DMA engine vs. CPU compression vs. NVMe controller); block count >> pipeline depth
- avoid_when: Only 1-2 blocks to transfer (pipeline fill/drain overhead dominates); insufficient buffer memory for multiple in-flight chunks; correctness requires all-or-nothing semantics per block

**Scatter-gather (fan-out writes, fan-in reads)**
- prefer_when: Distributed storage where blocks map to different remote nodes; striped storage requiring parallel access to multiple devices; high per-target latency but low per-target bandwidth
- avoid_when: Single-device target; ordering constraints between blocks; limited connection/socket budget; scatter amplifies failure probability

**Evidence:** Mooncake's disaggregated cache leverages the GPU cluster's underutilized CPU, DRAM, and SSD in an overlapped pipeline: prefill nodes push KV blocks to the distributed cache while decode nodes pull from it concurrently, achieving 525% throughput increase in overloaded scenarios (FAST 2025). InfiniGen overlaps current-layer GPU compute with speculative prefetch of next-layer KV blocks from host memory, achieving 3× speedup over non-overlapped offloading baselines (OSDI 2024).

### 4.3 Buffer Ownership Model

**Copy semantics (each stage gets its own copy)**
- prefer_when: Stages operate at different rates and need independent lifetimes; different stages apply different transformations (compression, checksumming); correctness requires immutable snapshots during transfer
- avoid_when: Block sizes are large (5-10 MB each) and copies double memory pressure; bandwidth between stages is the bottleneck (copy adds latency); GPU memory is scarce

**Zero-copy / ownership transfer (buffer moves between stages)**
- prefer_when: Large blocks where copy cost is material; stages execute sequentially (no concurrent access needed); DMA engines can pin and transfer user buffers directly
- avoid_when: Multiple downstream consumers need simultaneous access; buffer pool fragmentation risk when different stages free at different rates; device requires specific memory alignment incompatible with prior stage

**Reference-counted shared buffers**
- prefer_when: Prefix sharing means the same block feeds multiple concurrent reads; copy-on-write needed for blocks being read while simultaneously being demoted; multiple pipeline paths share a common buffer pool
- avoid_when: Reference counting overhead matters at very high op rates; single-consumer paths where simpler ownership suffices; real-time guarantees needed (refcount GC adds jitter)

**Evidence:** vLLM PagedAttention uses reference-counted physical blocks—when a shared prefix block is referenced by N sequences, no copies are made until one sequence diverges (copy-on-write), enabling 2-4× throughput via higher effective batch size (SOSP 2023). SGLang's RadixAttention extends this to tree-structured sharing across the full radix trie (6.4× throughput on multi-turn workloads).

### 4.4 Backpressure Strategy

**Block (stop producer until consumer catches up)**
- prefer_when: Correctness requires no data loss; buffer pool is exhausted; consumer slowdown is transient (e.g., brief NVMe GC pause)
- avoid_when: Producer is latency-critical (blocking stalls decode token generation); deadlock risk if producer and consumer share resources; sustained rate mismatch where blocking creates cascading delays

**Spill to slower tier (overflow to DRAM/SSD when fast path is full)**
- prefer_when: SLO requires producer to never block; temporary spill is recoverable; multi-tier architecture already supports seamless demotion
- avoid_when: Slower tier is also saturated; spill creates ordering violations that complicate recovery; metadata overhead of tracking spilled blocks exceeds benefit

**Admission control (reject or recompute rather than queue)**
- prefer_when: Sustained overload where queuing just delays the inevitable; recompute cost is known and bounded; SLO guarantees require bounded latency even under load
- avoid_when: Recompute cost exceeds storage fetch cost; rejected work has no fallback path; load spikes are brief and absorption is cheaper than rejection

**Credit-based flow control (consumer grants credits, producer sends only when credited)**
- prefer_when: Network transfers between nodes with variable latency; multiple producers targeting one consumer (fairness needed); fine-grained rate matching required
- avoid_when: Single-device local transfers where simpler blocking suffices; credit exchange overhead is material relative to transfer time; very short bursts where credit negotiation can't converge

**Evidence:** Mooncake implements prediction-based early rejection as backpressure—requests predicted to miss SLOs are rejected before consuming prefill resources, which prevents pipeline stalls in the downstream KV transfer path (FAST 2025). DistServe places prefill and decode on separate GPUs with bandwidth-aware placement, implicitly managing backpressure by sizing the inter-instance link to the expected KV transfer rate (OSDI 2024).

### 4.5 Compression Placement

**Compress-before-send (in the producer)**
- prefer_when: Network bandwidth is the bottleneck; compression ratio is high (>2×); spare CPU/GPU cycles available at the producer; latency budget accommodates compression time
- avoid_when: Compression time exceeds bandwidth savings; producer is compute-bound (prefill phase); decompression at consumer adds latency to critical path

**Send-raw (no compression)**
- prefer_when: Network bandwidth exceeds KV generation rate; producer has no spare cycles; consumer needs immediate access without decode overhead; local transfers (PCIe, NVLink)
- avoid_when: Network is shared/congested; block sizes are large and compression ratio is known-good; remote multi-hop transfers where bytes saved compound

**Adaptive compression (vary level based on available bandwidth)**
- prefer_when: Network bandwidth fluctuates (shared infrastructure, cloud); mixed workloads where some transfers are latency-critical and others are bulk; the system has measured compression ratio for live KV data
- avoid_when: Fixed dedicated links with stable bandwidth; compression codec doesn't support variable levels efficiently; added complexity of adaptation outweighs gain

**Evidence:** CacheGen adapts compression level per chunk based on real-time bandwidth probing, achieving 3.5-4.3× KV cache size reduction with 3.2-3.7× total delay reduction and negligible quality loss (SIGCOMM 2024). When bandwidth drops, it compresses more aggressively or falls back to selective GPU recomputation of specific layers.

## 5. Coupled Constraints

| This decision | Interacts with | Mechanism |
|---------------|----------------|-----------|
| Queue depth | Buffer pool size | Each in-flight op consumes one buffer; deep queues drain the pool |
| Queue depth | Tail latency | Deeper queues increase queuing delay variance (M/M/c) |
| Pipeline overlap | Buffer count | Double-buffering needs 2× memory at each stage boundary |
| Pipeline overlap | Block size | Small blocks have high pipeline fill/drain ratio; overlap helps less |
| Backpressure | Scheduler admission | Backpressure signals should propagate to request admission to prevent cascading |
| Backpressure | Eviction policy | Under backpressure, eviction must free buffers, not just logical cache entries |
| Compression | CPU budget | Compression competes with model layers running on CPU (some MoE decode paths) |
| Compression | Buffer sizing | Compressed output is variable-size; fixed buffers waste space or require realloc |
| Scatter-gather | Placement policy | Fan-out degree depends on where blocks are distributed |
| Zero-copy | Alignment requirements | GPU DMA requires page-aligned buffers; NVMe requires 512B/4KB alignment |
| Concurrency | Request isolation | High concurrency on shared paths means one request's bulk transfer delays another's critical fetch |

## 6. Failure Modes

### 6.1 Underpipelining
**Symptom**: Device utilization far below rated bandwidth; large idle gaps between transfers.
**Cause**: Queue depth too shallow for device's internal parallelism, or pipeline stages are serialized unnecessarily.
**Consequence**: TTFT/TPOT SLOs missed because KV blocks arrive slower than the model can consume them.

### 6.2 Buffer Exhaustion
**Symptom**: Sudden throughput cliff; producer blocks waiting for free buffers; OOM crashes.
**Cause**: Queue depth or pipeline width set too deep without corresponding buffer pool reservation.
**Consequence**: Cascading stall—blocked producers prevent request completion, increasing memory pressure further.

### 6.3 Head-of-Line Blocking
**Symptom**: Latency-critical decode fetches stuck behind bulk prefill writebacks in a shared queue.
**Cause**: Single FIFO queue serving both latency-critical and throughput-optimal traffic.
**Consequence**: TPOT violations for active generation requests while the device is busy with background writeback.

### 6.4 Backpressure Oscillation
**Symptom**: Throughput alternates between bursts and stalls; sawtooth pattern in device utilization.
**Cause**: Backpressure thresholds set without hysteresis (high watermark = low watermark), or adaptation faster than the system's settling time.
**Consequence**: Wasted bandwidth during stall phases; unpredictable latency.

### 6.5 Compression Bottleneck
**Symptom**: Network link utilization drops despite available bandwidth; CPU fully saturated.
**Cause**: Compress-before-send chosen when compression throughput < link bandwidth.
**Consequence**: Compression becomes the pipeline bottleneck, slower than sending uncompressed.

### 6.6 Scatter Amplification
**Symptom**: Tail latency grows with fan-out degree; one slow node dominates end-to-end time.
**Cause**: Scatter-gather where completion requires all shards; one straggler blocks the batch.
**Consequence**: Effective latency = max(shard latencies), which grows as O(log N) for N shards under exponential tail.

### 6.7 Zero-Copy Pinning Contention
**Symptom**: GPU memory fragmentation; DMA transfers fail intermittently; allocator contention.
**Cause**: Too many pinned buffers preventing the memory allocator from compacting or reallocating for compute.
**Consequence**: Model execution slowed by reduced available GPU memory; transfers succeed but compute degrades.

## 7. Hypotheses the Agent Can Generate

From this knowledge, the agent can form testable hypotheses such as:

1. "Increasing NVMe queue depth from 4 to 32 will improve writeback throughput by >2× because the current depth underutilizes the device's internal parallelism" (test: sweep queue depth, measure bandwidth plateau).

2. "Double-buffering the prefill→decode KV handoff path will hide transfer latency because transfer time ≈ one decode iteration time" (test: measure overlap efficiency with 2 vs. 1 buffers).

3. "Compressing KV blocks before network transfer will reduce TTFT because compression_time + transfer_compressed < transfer_raw for our measured 3.8× ratio" (test: compare end-to-end TTFT with and without compression).

4. "Separating the IO queue into priority lanes (decode-fetch vs. writeback) will reduce p99 TPOT by >50% because it eliminates head-of-line blocking" (test: paired experiment with shared vs. split queues under mixed load).

5. "The current backpressure threshold is set too aggressively—raising the high watermark from 0.6 to 0.8 will increase sustained throughput without buffer exhaustion" (test: sweep watermark, measure throughput and OOM events).

6. "Scatter-gather with redundant reads (read from 2 of 3 replicas, take first response) will cut tail latency at the cost of 2× read amplification" (test: compare p99 with speculative vs. non-speculative reads).

7. "The pipeline is CPU-bound on compression—moving to GPU-accelerated encoding will shift the bottleneck to network bandwidth and unlock 2× transfer rate" (test: profile pipeline stages, replace CPU codec with GPU codec, remeasure).

8. "InfiniGen-style speculative prefetch will hide host→GPU transfer latency for offloaded KV because current-layer compute time (2-5 ms) exceeds PCIe transfer time for the predicted subset" (test: measure prediction accuracy × prefetch coverage vs. compute window).

## 8. Experiments and Falsifiers

### 8.1 Queue Depth Saturation Sweep
**Hypothesis**: Queue depth N saturates device bandwidth.
**Method**: Fix block size, sweep queue_depth ∈ {1, 2, 4, 8, 16, 32, 64}. Measure throughput (MB/s) and p99 latency at each depth.
**Falsifier**: If throughput plateaus at depth < N, the hypothesis is false—a lower depth suffices.
**Control**: Single isolated device, no competing IO.

### 8.2 Pipeline Overlap Efficiency
**Hypothesis**: Overlapping stage A with stage B achieves near-2× speedup over serial.
**Method**: Transfer M blocks serially (time T_serial) then with double-buffered overlap (time T_overlap). Measure overlap_efficiency = T_serial / T_overlap.
**Falsifier**: If overlap_efficiency < 1.3, the stages share a bottleneck resource and overlap doesn't help.
**Control**: Ensure stages use different physical resources (e.g., DMA engine vs. CPU compression).

### 8.3 Backpressure Threshold Sensitivity
**Hypothesis**: Current watermarks cause oscillation, and wider hysteresis stabilizes throughput.
**Method**: Under sustained overload, measure throughput coefficient of variation with (high=0.8, low=0.5) vs. (high=0.8, low=0.8) vs. (high=0.9, low=0.6).
**Falsifier**: If CoV is low (<0.1) at all settings, backpressure is not the source of instability.

### 8.4 Compression Break-Even Point
**Hypothesis**: Compression reduces end-to-end transfer time on path X.
**Method**: Measure t_compress at each compression level. Measure raw bandwidth on path X. Compute break-even: if t_compress < block_bytes × (1 - ratio) / bw, compression wins.
**Falsifier**: If t_compress exceeds the bandwidth savings at all achievable ratios, send raw.

### 8.5 Priority Queue vs. FIFO for Mixed Traffic
**Hypothesis**: Strict priority for decode-path fetches reduces p99 TPOT.
**Method**: Paired experiment—same workload with shared FIFO vs. dual-queue (high-priority decode reads, low-priority writeback). Measure TPOT p99 and writeback completion time.
**Falsifier**: If decode reads are never delayed by writebacks (FIFO p99 ≈ priority p99), the workload doesn't exhibit head-of-line blocking.

### 8.6 Speculative Prefetch Coverage
**Hypothesis**: Predicting next-layer KV needs during current-layer compute hides fetch latency.
**Method**: Log prediction accuracy (fraction of actually-needed blocks that were prefetched) and wasted bandwidth (prefetched but unused). Measure decode latency with/without prefetch.
**Falsifier**: If prediction accuracy < 60% or compute_time < fetch_time for the predicted set, prefetch doesn't hide latency and wastes bandwidth.

### 8.7 Buffer Pool Sizing Experiment
**Hypothesis**: Current buffer pool is undersized, causing frequent backpressure activation.
**Method**: Instrument backpressure trigger count over time. Double buffer pool size, repeat. If trigger count drops >80% and throughput increases, pool was undersized.
**Falsifier**: If increasing buffer pool has no effect on trigger count, backpressure is not buffer-caused (likely downstream rate limit).

## 9. Production Evidence

### 9.1 Mooncake — Pipeline Disaggregation at Scale
**System**: Mooncake (Kimi/Moonshot AI production serving platform)
**Problem**: Colocated prefill-decode creates memory contention that limits batch size and wastes GPU cycles during KV transfers.
**Mechanism**: Disaggregated architecture separates prefill and decode clusters, using underutilized CPU/DRAM/SSD as a distributed KV cache layer. A KV-centric scheduler coordinates producer-consumer flow between clusters, with prediction-based early rejection as backpressure under overload.
**Result**: 525% throughput increase in simulated overloaded scenarios; 75% more requests handled in production (Kimi service) while maintaining SLOs.
**Lesson**: The pipeline between prefill (producer) and decode (consumer) is the critical path—overprovisioning this transfer link and implementing admission-based backpressure yields more throughput than optimizing either phase alone.
**Source**: Qin et al., "Mooncake: A KVCache-Centric Disaggregated Architecture for LLM Serving," FAST 2025.

### 9.2 InfiniGen — Speculative Prefetch Overlap
**System**: InfiniGen (KV cache management for offloading-based inference)
**Problem**: Offloaded KV cache in host memory requires bulk transfers to GPU before each attention layer, serializing compute and IO.
**Mechanism**: Uses lightweight "minimal rehearsal" during current-layer compute to speculate which KV entries will be important for the next layer. Initiates prefetch of only those entries concurrently with ongoing GPU computation, creating a compute-prefetch pipeline.
**Result**: 3× improvement over prior offloading methods with substantially better model accuracy (no information loss, unlike eviction-based approaches).
**Lesson**: Selective prefetch (transfer only predicted-important entries) combined with compute overlap transforms an IO-bound pipeline into a compute-bound one. The prediction accuracy need not be perfect—even 70-80% coverage hides most transfer latency.
**Source**: Lee et al., "InfiniGen: Efficient Generative Inference of Large Language Models with Dynamic KV Cache Management," OSDI 2024.

### 9.3 FlexGen — LP-Optimized Pipeline Depth
**System**: FlexGen (single-GPU inference via CPU/SSD offloading)
**Problem**: OPT-175B requires far more memory than a single GPU provides; naive offloading serializes compute and transfer.
**Mechanism**: Solves a linear programming problem to find the optimal batch size, pipeline schedule, and tensor placement across GPU/CPU/disk. Uses large batch sizes (144) to keep all three tiers simultaneously active—GPU computes on batch slice N while CPU receives slice N+1's weights and SSD streams slice N+2's KV cache.
**Result**: 1 token/s generation throughput for OPT-175B on a single 16GB GPU—previously impossible without model parallelism.
**Lesson**: The number of pipeline stages and the batch size that fills them must be co-optimized. Effective concurrency across a 3-tier hierarchy requires that batch_size × block_size exceeds the bandwidth-delay product of the slowest tier.
**Source**: Sheng et al., "FlexGen: High-Throughput Generative Inference of Large Language Models with a Single GPU," ICML 2023.

### 9.4 CacheGen — Adaptive Compression Pipeline
**System**: CacheGen (KV cache compression and streaming)
**Problem**: Transferring full KV caches over the network for context reuse creates latency proportional to cache size, making reuse unattractive for large contexts.
**Mechanism**: Custom tensor encoder exploits KV cache distributional properties. Adapts compression level per-chunk based on real-time bandwidth probing. Falls back to GPU recomputation when compression at acceptable quality cannot keep up with bandwidth constraints—an adaptive backpressure that trades compute for IO.
**Result**: 3.5-4.3× size reduction; 3.2-3.7× total delay reduction (including encode + transfer + decode); negligible quality loss.
**Lesson**: Compression placement should be adaptive, not fixed. The pipeline should monitor downstream bandwidth and adjust compression level per-chunk, treating recomputation as the ultimate backpressure valve when no compression level fits the time budget.
**Source**: Liu et al., "CacheGen: KV Cache Compression and Streaming for Fast Large Language Model Serving," SIGCOMM 2024.

### 9.5 DistServe/Splitwise — Bandwidth-Aware Phase Placement
**System**: DistServe (disaggregated prefill/decode serving)
**Problem**: Splitting prefill and decode onto separate GPUs requires KV cache transfer between them; placement determines whether this transfer becomes a bottleneck.
**Mechanism**: Co-optimizes resource allocation and parallelism strategy per phase. Places phases according to cluster bandwidth topology to minimize inter-instance communication cost. The KV transfer pipeline uses fast back-plane interconnects (NVLink, InfiniBand) with placement ensuring the transfer fits within TTFT budget.
**Result**: 7.4× more requests or 12.6× tighter SLO compared to state-of-the-art colocated systems while meeting latency constraints for >90% of requests.
**Lesson**: Pipeline concurrency is meaningless if the pipeline is placed across a bandwidth-starved link. Placement policy must account for the bytes_to_transfer / available_bandwidth ratio and co-locate prefill-decode pairs within the same high-bandwidth domain when KV size is large.
**Source**: Zhong et al., "DistServe: Disaggregating Prefill and Decoding for Goodput-optimized Large Language Model Serving," OSDI 2024.

### 9.6 vLLM PagedAttention — Concurrency Through Memory Efficiency
**System**: vLLM (production LLM serving)
**Problem**: KV cache memory fragmentation limits batch size, which limits the number of concurrent sequences that can generate IO demand.
**Mechanism**: Paged memory management with reference-counted blocks enables near-zero waste and copy-on-write sharing. This increases effective batch size, which in turn increases the IO concurrency (more sequences = more prefetch/writeback operations that can overlap).
**Result**: 2-4× throughput improvement; improvements more pronounced with longer sequences and larger models where memory pressure is highest.
**Lesson**: IO concurrency is often gated by how many sequences fit in memory simultaneously. Memory efficiency improvements that increase batch size have a multiplicative effect on effective IO parallelism—more concurrent sequences mean more opportunities to overlap fetch with compute across different requests.
**Source**: Kwon et al., "Efficient Memory Management for Large Language Model Serving with PagedAttention," SOSP 2023.

### 9.7 SGLang RadixAttention — Shared-Prefix Pipeline Efficiency
**System**: SGLang (structured generation runtime)
**Problem**: Multi-turn chat, few-shot learning, and agentic workloads reuse large prefixes; without sharing, each request redundantly transfers/computes the same KV blocks.
**Mechanism**: Radix tree tracks all live prefix paths. When multiple requests share a prefix, only one copy exists in the cache, and the pipeline only transfers the unique suffix. This reduces effective IO volume proportional to the sharing ratio.
**Result**: 6.4× higher throughput on multi-turn and structured workloads.
**Lesson**: Effective pipeline concurrency is not just about how fast you transfer—it's about how much you need to transfer. Deduplication at the block level via tree-structured sharing can reduce IO demand by 3-10× in production workloads with high prefix commonality, making the pipeline "fast enough" without hardware changes.
**Source**: Zheng et al., "SGLang: Efficient Execution of Structured Language Model Programs," 2024.

---

## Implications for KV Block Storage

1. **Queue depth must be a tunable, not a constant.** Different transfer paths (local NVMe vs. RDMA to remote cache vs. PCIe to GPU) saturate at different depths. The storage system should expose per-path queue depth as an optimization parameter and provide bandwidth-vs-depth curves for the agent to reason about.

2. **Priority-aware IO scheduling is mandatory.** A single FIFO queue mixing decode-critical fetches with background writeback will always create head-of-line blocking under load. The storage engine needs at minimum two priority lanes—or the scheduler must ensure latency-critical and bulk operations never share the same queue.

3. **Buffer pools must be carved out at design time, not borrowed from compute.** Zero-copy transfer requires pre-pinned, pre-aligned buffers. If the storage layer allocates from the same pool as KV compute memory, deep pipelines will steal from active sequences. The budget equation (stages × depth × block_size) determines the reservation.

4. **Backpressure must propagate to the request scheduler.** When the storage pipeline saturates, the serving scheduler must know—either to reduce admission rate, redirect to nodes with free pipeline capacity, or accept recomputation as the fallback. Silent queuing at the storage layer leads to unbounded TTFT growth.

5. **Compression should be an inline pipeline stage, not a separate system.** The CacheGen evidence shows adaptive compression is most effective when tightly integrated into the transfer pipeline (per-chunk level selection based on real-time bandwidth). Treating compression as a pre-processing step loses the ability to adapt.

6. **The pipeline design depends on block size.** A 5 MB block (Llama-70B, 16 tokens) has very different pipeline economics than a 128 KB block (Mixtral, 16 tokens). The storage system should support configurable pipeline parameters per model/block-size combination.

7. **Effective concurrency is a system-level metric.** It emerges from memory efficiency (batch size), prefix sharing (IO reduction), queue depth (device utilization), and pipeline overlap (latency hiding)—not from any single parameter. The agent must measure all four dimensions to understand whether more parallelism will help.

Good, I can see the format of existing files. Now I have enough context to write the complete domain reference file.

Here is the full domain reference file:

```markdown
---
id: cache-value-and-recompute
decision: Whether to retain a cached KV block or let it be evicted and recomputed on next use
answers:
  - When is a cached block worth keeping vs recomputing?
  - What is the economic value of a specific cached block?
  - How do you estimate the probability a block will be reused?
  - What compute cost does a cache hit actually save?
  - When does storing a block cost more than recomputing it?
inputs:
  - reuse probability (from prefix tree hit rate or frequency counter)
  - recompute cost in FLOPs or GPU-seconds per block
  - storage + transfer cost per block per unit time
  - current memory pressure (free capacity in each tier)
  - time since last access (recency signal)
  - block size in bytes
  - request arrival rate and prefix distribution
  - SLO headroom (time-to-first-token budget remaining)
owns: ['economic value model', 'reuse probability', 'saved compute']
excludes:
  - eviction algorithm mechanics (see tier-policy-and-eviction)
  - physical placement and routing (see distributed-kv-ownership)
  - IO pipeline depth and concurrency (see data-movement-concurrency)
  - block geometry and sizing (see kv-footprint-and-lifecycle)
  - hardware specifications
  - attention kernel internals
related:
  - tier-policy-and-eviction
  - kv-footprint-and-lifecycle
  - workload-to-storage-io
  - distributed-kv-ownership
  - bottleneck-and-measurement
---

## 1. Decision Being Made

The optimization agent must decide, for each cached KV block:

1. **Keep or evict**: Given finite storage across tiers (GPU HBM, host DRAM, SSD), which blocks justify their residency cost?
2. **Cache or recompute**: When a previously-evicted block is needed again, is the recompute path cheaper than having stored it?
3. **Value ranking**: Among all cached blocks, which provides the highest marginal value per byte of storage consumed?

This is fundamentally an economics problem. A block's value is the product of its reuse probability and the compute cost it saves on hit, minus the ongoing cost of storing it. The agent must maintain or estimate these quantities in real-time to make admission and retention decisions.

The decision is distinct from *where* a block should live (tier-policy-and-eviction) or *how* to move it (data-movement-concurrency). This file owns the value model that those systems consume as input.


## 2. Mental Model and Equations

### 2.1 Block Value Equation

The marginal value of retaining block `b` is:

```
V(b) = P_reuse(b) × C_recompute(b) − C_store(b, Δt)
```

Where:
- `P_reuse(b)` = probability block is accessed again before natural expiration
- `C_recompute(b)` = cost (in GPU-seconds or FLOPs) to regenerate from scratch
- `C_store(b, Δt)` = amortized cost of holding the block for time interval Δt

A block is worth keeping when `V(b) > 0`. Among candidates for eviction, evict the block with the lowest `V(b)`.

### 2.2 Recompute Cost

For a transformer with `L` layers, `H` KV heads, head dimension `d`, and block size `T` tokens:

```
C_recompute(b) = L × (2 × H × d × T × S_prefill) × FLOP_cost
```

Where `S_prefill` is the total sequence length at the block's position (attention is O(T × S) per layer). Key insight: blocks at the *end* of long sequences are much more expensive to recompute than blocks near the start, because attention cost grows with position.

For prefix blocks (position 0..T), recompute cost is relatively cheap:
```
C_recompute_prefix ≈ L × 2 × H × d × T² × FLOP_cost
```

For blocks at position P in a sequence of total length S:
```
C_recompute_at_P ≈ L × 2 × H × d × T × P × FLOP_cost
```

This creates a value gradient: later blocks in a sequence are inherently more valuable to cache because their recompute is more expensive.

### 2.3 Reuse Probability

Reuse probability depends on the block's position in the prefix tree:

```
P_reuse(b) = f(frequency, recency, prefix_depth, sharing_degree)
```

Empirical signals:
- **System prompt blocks**: P_reuse ≈ 1.0 (shared across all requests for a deployment)
- **Common prefix blocks**: P_reuse = (requests sharing this prefix) / (total requests) per time window
- **Session-specific blocks**: P_reuse = P(user returns within TTL) × P(same conversation context)
- **Unique decode blocks**: P_reuse ≈ 0 (single-use, never retain)

### 2.4 Storage Cost

```
C_store(b, Δt) = bytes(b) × cost_per_byte_second(tier) × Δt
```

Where `cost_per_byte_second` reflects the opportunity cost of the tier:
- GPU HBM: highest (displaces active batch slots)
- Host DRAM: moderate (displaces other system uses)
- SSD: lowest (but adds transfer latency on hit)

### 2.5 GDSF-style Composite Score

GreedyDual-Size-Frequency (GDSF) provides a practical composite:

```
Priority(b) = (frequency × cost_to_fetch) / size(b) + age_factor
```

This naturally handles size-aware caching: large blocks need proportionally higher value to justify residency. In the KV context, `cost_to_fetch` becomes `C_recompute` for compute-backed blocks or transfer latency for tiered blocks.

### 2.6 Break-Even Condition

A block breaks even when stored for time `T_be`:

```
T_be = C_recompute(b) × P_reuse(b) / (bytes(b) × cost_per_byte_second)
```

If the expected inter-access time exceeds `T_be`, evict. If below, retain.


## 3. Required Observations

Before making keep-vs-recompute decisions, the agent must measure:

| Observation | How to obtain | Why needed |
|---|---|---|
| Block access frequency | Counter per block (or Bloom/CountMin sketch) | Estimates P_reuse |
| Time since last access | Timestamp per block | Recency signal for decay |
| Prefix tree hit rate | Track radix tree lookups vs misses | Aggregate reuse signal |
| Prefill compute time per token | Instrument prefill kernel | Converts to C_recompute |
| Tier occupancy | Memory allocator stats per tier | Pressure signal |
| Request arrival rate | Scheduler queue depth | Scales reuse probability |
| Prefix distribution entropy | Track unique vs shared prefix lengths | Low entropy → high reuse |
| TTFT SLO headroom | (SLO target − current TTFT) | Determines tolerance for recompute |
| Block position in sequence | Metadata per block | Value gradient factor |
| Transfer bandwidth utilization | IO counters per tier boundary | Opportunity cost of fetch |


## 4. Alternatives with Prefer/Avoid

### 4.1 Always-Cache (Retain Until Memory Pressure)

**Strategy**: Cache every computed block; only evict under pressure using LRU/LFU.

- **prefer_when**: Storage is abundant relative to working set; recompute is expensive (long sequences, large models); workload has high temporal locality
- **avoid_when**: Working set far exceeds cache capacity; workload is random-access over large prefix space; storage tier is expensive (HBM) and displaces batch slots

### 4.2 Value-Weighted Retention (GDSF / ARC-style)

**Strategy**: Score each block by `(reuse_probability × saved_compute) / size`; admit and evict based on score.

- **prefer_when**: Heterogeneous block values (mix of shared system prompts and unique sessions); multiple tiers with different costs; measurable frequency signals exist
- **avoid_when**: All blocks have similar reuse probability (uniform workload); overhead of per-block scoring exceeds savings; insufficient history to estimate frequency (cold start)

### 4.3 Prefix-Aware Selective Caching (RadixAttention Model)

**Strategy**: Cache based on position in the prefix tree. Shared prefixes get infinite TTL; per-session blocks get bounded TTL; decode-only blocks are never cached.

- **prefer_when**: Workload has high prefix sharing (chatbots with system prompts, RAG with shared documents); prefix tree structure is maintained; clear sharing hierarchy exists
- **avoid_when**: Requests have unique prefixes (no sharing); prefix tree maintenance cost exceeds benefit; blocks cannot be cleanly decomposed by prefix boundary

### 4.4 Recompute-Preferred (Minimal Caching)

**Strategy**: Only cache blocks where recompute is prohibitively expensive or violates SLO. Default to recompute.

- **prefer_when**: GPU compute is abundant but memory is scarce; sequences are short (recompute is cheap); TTFT SLO is generous; workload is bursty with low reuse
- **avoid_when**: Recompute would violate SLO constraints; prefill is the bottleneck; long-context workloads where recompute cost is superlinear

### 4.5 Compressed Retention (CacheGen Model)

**Strategy**: Keep blocks but compress them 3-4x. Pay decompression cost on hit instead of full recompute cost.

- **prefer_when**: Storage is the bottleneck but bandwidth exists for decompression; recompute cost >> decompression cost; blocks have compressible distributions (common in middle layers); quality degradation from lossy compression is tolerable
- **avoid_when**: Decompression latency violates SLO; compression ratio is poor for this model's KV distribution; GPU compute for decompression competes with inference; lossless requirement (safety-critical outputs)

### 4.6 Speculative Prefetch (InfiniGen Model)

**Strategy**: Don't keep everything in fast tier. Use cheap speculation to predict which blocks will be needed, prefetch selectively from slow tier.

- **prefer_when**: Only a fraction of cached blocks are actually accessed per decode step (sparse attention patterns); slow tier has adequate bandwidth for selective fetch; prediction accuracy is high (>80%); working set exceeds fast tier capacity
- **avoid_when**: Dense attention patterns (every cached token is used); prediction is unreliable; slow tier bandwidth is already saturated; latency of misprediction is unacceptable


## 5. Coupled Constraints

### 5.1 Value ↔ Tier Placement

A block's optimal tier depends on its value score. High-value blocks (high P_reuse, high C_recompute) justify HBM residency. Low-value blocks that still beat eviction go to DRAM or SSD. The value model feeds tier-policy-and-eviction decisions.

### 5.2 Value ↔ Batch Size

Every byte of HBM used for caching is a byte not available for batch slots. The marginal value of a cached block must exceed the throughput gain from one additional batch slot:

```
V(b) > marginal_throughput_gain × revenue_per_token
```

This creates a dynamic threshold that rises under load.

### 5.3 Value ↔ Prefill Scheduling

If the system has idle prefill capacity (Mooncake-style disaggregation), recompute cost effectively drops—making eviction cheaper. The value model must reflect *current* compute availability, not static FLOP counts.

### 5.4 Value ↔ Compression

Compression changes the storage cost term. A block compressed 4x has 1/4 the storage cost, shifting the break-even point. But decompression adds to fetch latency, which may reduce the effective value of a hit.

### 5.5 Value ↔ Sharing Degree

A block shared by N concurrent sessions has effective value multiplied by N (one eviction causes N cache misses). Sharing degree must factor into retention decisions:

```
V_shared(b) = N_sharers × P_reuse(b) × C_recompute(b) − C_store(b, Δt)
```

### 5.6 Value ↔ SLO Tightness

Under tight TTFT SLOs, the "cost" of a miss is not just compute but potential SLO violation. This inflates effective C_recompute by a penalty factor proportional to SLO tightness:

```
C_effective(b) = C_recompute(b) + penalty × max(0, predicted_latency − SLO_budget)
```


## 6. Failure Modes

### 6.1 Over-Caching (Value Overestimation)

**Symptom**: Cache is full of rarely-reused blocks; cache hit rate is high but only because stale entries match.  
**Cause**: P_reuse estimated from historical frequency that no longer reflects current workload.  
**Consequence**: Useful blocks are evicted to make room for stale ones. Batch size shrinks due to memory pressure. Throughput drops despite nominally high hit rate.

### 6.2 Under-Caching (Value Underestimation)

**Symptom**: Excessive recompute; prefill GPU utilization is high while decode GPUs starve waiting for KV.  
**Cause**: Value model ignores sharing degree, or uses too-short frequency windows, or applies uniform eviction ignoring the position-dependent recompute cost.  
**Consequence**: TTFT rises, SLOs violated, GPU cycles wasted on redundant prefill.

### 6.3 Thrashing

**Symptom**: Block is evicted then immediately recomputed, repeatedly.  
**Cause**: Working set slightly exceeds capacity; LRU-style policies cycle through blocks without frequency awareness.  
**Consequence**: Worst of both worlds—paying storage cost AND recompute cost. ARC-style adaptive policies or admission filters (TinyLFU) specifically address this.

### 6.4 Value Inversion Under Load

**Symptom**: System retains low-value blocks during load spikes because eviction decisions lag behind demand.  
**Cause**: Value scores computed at admission time are not updated as conditions change (batch pressure rises, arrival rate spikes).  
**Consequence**: The blocks that seemed valuable at low load become liabilities at high load. Need dynamic re-scoring or lazy eviction with pressure-triggered sweeps.

### 6.5 Cold-Start Mis-Admission

**Symptom**: New deployment has no frequency data; admits everything or nothing.  
**Cause**: Value model requires historical access patterns that don't exist yet.  
**Consequence**: Either wastes capacity on one-shot blocks (over-admit) or recomputes shared prefixes repeatedly (under-admit). TinyLFU's Doorkeeper Bloom filter addresses this by requiring a second access before admission.

### 6.6 Compression Quality Degradation

**Symptom**: Model output quality drops subtly after enabling compressed KV retention.  
**Cause**: Lossy compression in KV values introduces attention score drift that accumulates over long sequences.  
**Consequence**: Serving quality degrades without obvious metrics alerting—requires A/B testing on downstream task quality, not just cache performance metrics.


## 7. Hypotheses the Agent Can Generate

From this knowledge, an optimization agent can formulate testable hypotheses:

1. **"Blocks at position > P have recompute cost exceeding 2x the storage cost at current load"** — Test by measuring prefill time vs storage cost for blocks at different positions.

2. **"System prompt blocks are accessed by >90% of requests and should be pinned in HBM"** — Test by tracking access counts per prefix depth.

3. **"The working set exceeds tier-1 capacity by X%, causing Y% of evictions to thrash"** — Test by measuring evict-then-reload cycles vs total evictions.

4. **"Compressing middle-layer KV blocks 4x would free Z GB without quality loss"** — Test by compressing and measuring downstream task accuracy.

5. **"Admission filtering (requiring 2+ accesses before caching) would reduce one-shot pollution by X%"** — Test by shadowing an admission filter alongside current policy.

6. **"Under current arrival rate, the break-even TTL for session blocks is T seconds"** — Compute from measured P_reuse decay curve and storage cost.

7. **"Disaggregated prefill capacity makes recompute 3x cheaper than measured, shifting eviction threshold"** — Test by measuring effective recompute cost under current prefill cluster utilization.

8. **"Shared blocks have effective value Nx higher but are evicted at the same priority as unshared blocks"** — Test by correlating sharing degree with eviction rate.


## 8. Experiments and Falsifiers

### 8.1 Value Model Calibration

**Experiment**: Score all cached blocks with the value equation. Predict which blocks will be accessed in the next time window. Measure prediction accuracy.  
**Falsifier**: If predicted high-value blocks have <50% hit rate in the next window, the value model is miscalibrated.  
**Method**: Shadow scoring (compute scores but don't act on them) for one hour, then compare scores vs actual accesses.

### 8.2 Recompute Cost Dominance

**Experiment**: Measure actual GPU-seconds spent on redundant prefill (blocks that were previously computed, evicted, then recomputed).  
**Falsifier**: If redundant prefill accounts for <5% of total prefill compute, caching more aggressively has minimal benefit—focus elsewhere.  
**Method**: Tag blocks at computation time; on prefill, check if this exact block was computed before. Log the waste.

### 8.3 Thrashing Detection

**Experiment**: Track blocks that are evicted and reloaded within T seconds.  
**Falsifier**: If thrash rate is <1% of evictions, the eviction policy is adequate and ARC/TinyLFU complexity is unjustified.  
**Method**: Maintain a ghost cache (metadata only) of recently evicted blocks. On admission, check if the block is in the ghost cache.

### 8.4 Compression Break-Even

**Experiment**: Compress blocks at various ratios. Measure (decompression_time + fetch_time_compressed) vs (fetch_time_uncompressed) vs (recompute_time).  
**Falsifier**: If decompression_time > 50% of recompute_time, compression provides insufficient savings over just recomputing.  
**Method**: Benchmark decompression kernel; measure end-to-end TTFT under compressed vs uncompressed vs recompute paths.

### 8.5 Sharing Multiplier Validation

**Experiment**: Vary the sharing weight in the value equation (from 1x to Nx). Measure aggregate hit rate and TTFT across workload.  
**Falsifier**: If N-weighted scoring shows <5% TTFT improvement over uniform scoring, sharing degree isn't predictive of value in this workload.  
**Method**: A/B test eviction policies with and without sharing multiplier under production traffic.

### 8.6 Position-Value Gradient

**Experiment**: Partition blocks by sequence position. Measure cache hit value (saved compute) per position quartile.  
**Falsifier**: If blocks in the last quartile save <2x the compute of first-quartile blocks, the position gradient is too weak to justify position-aware eviction.  
**Method**: Instrument the prefill path to log position and compute time per recomputed block.

### 8.7 SLO-Aware Value Inflation

**Experiment**: Add SLO penalty term to value equation. Compare SLO violation rate against baseline.  
**Falsifier**: If SLO violations don't decrease under the penalty model, TTFT is dominated by factors other than cache misses (e.g., queuing, network).  
**Method**: Deploy penalty model on subset of traffic; compare P99 TTFT and SLO violation counts.


## 9. Production Evidence

### 9.1 Mooncake — Prefix-Aware KV Caching at Scale

**System**: Mooncake, production serving platform for Kimi (Moonshot AI), FAST 2025.  
**Problem**: Long-context workloads create massive KV caches; storing all of them in GPU memory limits batch size.  
**Mechanism**: Disaggregated KVCache-centric architecture. Prefill and decode separated into distinct clusters. Underutilized CPU, DRAM, and SSD resources form a distributed KV cache pool. Scheduler balances throughput with latency SLOs; prediction-based early rejection handles overload.  
**Result**: Up to 525% throughput increase in simulated long-context scenarios. Production system handles 75% more requests while meeting SLOs.  
**Lesson**: The value model must account for disaggregated compute—when prefill capacity is separate and plentiful, recompute cost is lower than in colocated systems, shifting the caching threshold.

### 9.2 SGLang RadixAttention — Prefix Tree Value Structure

**System**: SGLang serving engine (Zheng et al., 2024).  
**Problem**: Multi-turn chat, few-shot prompting, and RAG pipelines share long prefixes that are recomputed per request.  
**Mechanism**: Radix tree indexes all cached KV blocks by token prefix. LRU eviction operates on tree nodes, preserving shared prefixes longest. Automatic prefix matching eliminates manual prompt management.  
**Result**: Up to 6.4x higher throughput across agent tasks, logical reasoning, few-shot learning, and multi-turn chat compared to prior systems.  
**Lesson**: Prefix structure reveals reuse probability without per-block frequency tracking. Blocks deeper in a shared prefix path have higher value because they encode more compute and are reused by more requests.

### 9.3 vLLM PagedAttention — Memory Efficiency as Value Enabler

**System**: vLLM (Kwon et al., SOSP 2023).  
**Problem**: KV cache memory fragmented and duplicated across requests, wasting 60-80% of allocated space and limiting batch size.  
**Mechanism**: Virtual memory paging for KV blocks. Near-zero fragmentation enables flexible sharing within and across requests via copy-on-write.  
**Result**: 2-4x throughput improvement over FasterTransformer and Orca. More pronounced with longer sequences and larger models.  
**Lesson**: Before optimizing *which* blocks to keep, eliminate waste in *how* blocks are stored. Fragmentation artificially inflates storage cost in the value equation, making blocks appear less valuable than they are.

### 9.4 CacheGen — Compressed Retention Economics

**System**: CacheGen (Liu et al., SIGCOMM 2024).  
**Problem**: KV caches too large to transfer over network for disaggregated or edge serving.  
**Mechanism**: Custom tensor encoder exploiting KV distribution properties. Adaptive compression varies ratio across layers based on available bandwidth. Balances latency and quality.  
**Result**: 3.5-4.3x reduction in KV cache size. 3.2-3.7x reduction in total context fetch and processing delay. Negligible quality impact.  
**Lesson**: Compression changes the value equation: `C_store` drops by compression ratio, but `C_fetch` adds decompression overhead. The sweet spot is adaptive—compress more when bandwidth is scarce, less when latency-sensitive. Middle layers compress better than attention-critical first/last layers.

### 9.5 FlexGen — SSD as Lowest-Value Tier

**System**: FlexGen (Sheng et al., ICML 2023).  
**Problem**: Single-GPU inference for 175B parameter models requires offloading weights and KV cache to CPU/SSD.  
**Mechanism**: Linear programming over GPU/CPU/SSD placement. Batch size of 144 on a single 16GB GPU through aggressive offloading. 4-bit compression with negligible quality loss.  
**Result**: First system to achieve 1 token/s generation throughput for OPT-175B on a single 16GB GPU.  
**Lesson**: SSD tier makes economic sense for throughput-oriented, latency-insensitive workloads. The value threshold for SSD residency is much lower than for HBM—blocks with P_reuse > 0 and C_recompute > C_SSD_fetch can justify SSD caching.

### 9.6 InfiniGen — Selective Retrieval Validates Partial Value

**System**: InfiniGen (Lee et al., OSDI 2024).  
**Problem**: Long-context KV caches don't fit in GPU memory; fetching everything from offload tier is too slow.  
**Mechanism**: Speculates which KV entries are actually needed using minimal rehearsal with partial weight matrices. Prefetches only essential entries from CPU/SSD.  
**Result**: Up to 3x performance improvement over prior offloading-based methods with substantially better model accuracy.  
**Lesson**: Not all tokens in a cached block have equal value within a single decode step. Sparse attention means selective retrieval beats full-block fetch. Value is not just per-block but per-token within blocks—finer-grained value models unlock additional savings.

### 9.7 Splitwise — Disaggregation Changes Recompute Economics

**System**: Splitwise (Patel et al., ISCA 2024).  
**Problem**: Prefill (compute-heavy) and decode (memory-heavy) have opposite resource needs, wasting GPU capability.  
**Mechanism**: Separate prefill and decode onto different machines optimized for each phase. Transfer KV state via fast interconnects.  
**Result**: 1.4x throughput at 20% lower cost. 2.35x throughput within same cost and power budget.  
**Lesson**: Disaggregation reframes the keep-vs-recompute decision. When prefill runs on cheap, dedicated hardware, `C_recompute` drops—but `C_transfer` of KV state becomes the dominant cost. The value model must compare `C_recompute_on_prefill_node + C_transfer` against `C_store_on_decode_node`.

### 9.8 ARC — Adaptive Frequency/Recency Balance

**System**: ARC (Megiddo & Modha, FAST 2003).  
**Problem**: LRU fails on scan workloads; LFU fails on workload shifts. Static policies cannot adapt.  
**Mechanism**: Maintains two LRU lists (recent-once, recent-more-than-once) plus ghost caches for evicted entries. Dynamically adjusts the partition between frequency-favoring and recency-favoring based on which ghost cache is getting hit.  
**Result**: Continuously adapts to workload without tuning parameters. Matches or exceeds tuned static policies across diverse workloads.  
**Lesson**: The ghost cache concept directly applies to KV block caching: track metadata of evicted blocks cheaply, and use ghost hits to detect when eviction policy is making value errors (evicting blocks that are immediately needed again).

### 9.9 TinyLFU — Admission Gating Against Pollution

**System**: TinyLFU (Einziger et al., ACM ToS 2017), deployed in Caffeine (Java) and many production caches.  
**Problem**: New items with unknown value pollute the cache, evicting proven-valuable items.  
**Mechanism**: Approximate frequency sketch (Count-Min + Bloom filter) gates admission. New block only admitted if its estimated frequency exceeds the victim's. A Doorkeeper filter handles the cold-start problem by requiring a second access before tracking.  
**Result**: Near-optimal hit rate with O(8 bits) per tracked item. Eliminates scan/one-shot pollution without full frequency tracking.  
**Lesson**: For KV caching, TinyLFU's admission filter prevents single-use decode blocks from evicting shared prefix blocks. The Doorkeeper concept maps to: first request computes KV but doesn't cache; second request for same prefix triggers admission.


## 10. Implications for KV Block Storage

1. **Value is heterogeneous**: System prompt blocks (P_reuse ≈ 1, shared by all requests) are orders of magnitude more valuable than single-session decode blocks. Storage systems must expose per-block metadata to enable value-aware decisions.

2. **Position creates value gradients**: Blocks late in long sequences cost superlinearly more to recompute. A flat eviction policy (LRU by access time) ignores this structure. Storage metadata should include sequence position to enable position-weighted retention.

3. **Disaggregation shifts the equation**: When prefill runs on separate, potentially cheaper hardware (Mooncake, Splitwise), the recompute term shrinks. The storage system must accept dynamic `C_recompute` signals—not static config—so the value model adapts as cluster topology and load change.

4. **Compression is a value lever**: CacheGen's 3.5-4.3x compression means a block that was uneconomical to store at full size becomes viable compressed. The storage system should support per-block compression levels, trading quality margin for capacity.

5. **Ghost tracking prevents thrashing**: ARC's ghost cache and TinyLFU's admission filter are essential for KV workloads where one-shot decode blocks vastly outnumber shared prefix blocks. The storage layer should maintain evicted-block metadata (≤64 bytes per ghost) to detect and correct value estimation errors.

6. **Sharing degree is a first-class signal**: A block shared by N active sessions has Nx the effective value. Storage systems that track reference counts or sharing degree can feed this directly into retention decisions, preventing eviction of high-fan-out blocks under pressure.

7. **Multi-tier break-even differs per tier**: The value threshold for HBM residency is much higher than for SSD. A block evicted from HBM may still justify DRAM or SSD residency. The storage system should implement graduated demotion (HBM → DRAM → SSD → evict) with separate value thresholds per transition.

8. **Real-time signals beat static policy**: The value equation depends on load, sharing degree, SLO headroom, and compute availability—all of which change on second-to-minute timescales. Storage must expose these signals and accept re-scoring without stop-the-world pauses.

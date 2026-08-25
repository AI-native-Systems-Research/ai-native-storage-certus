I have sufficient context. Here's the complete domain reference file:

```markdown
---
id: tier-policy-and-eviction
decision: Which storage tier each KV block should occupy, when to move it, and when to discard it
answers:
  - Where should KV live, what should move or be evicted?
  - Which tier (GPU HBM, host DRAM, SSD, remote) should a block reside in?
  - When should a block be admitted to a higher tier?
  - Which block should be evicted when capacity is exhausted?
  - When should a block be promoted (lower→higher tier) or demoted (higher→lower)?
  - How should blocks be placed across a multi-node cluster?
  - When should garbage collection reclaim dead blocks?
inputs:
  - block value score V(b) from cache-value-and-recompute
  - per-tier capacity and current utilization
  - per-tier read/write bandwidth and latency
  - request arrival rate and prefix distribution
  - TTFT and TPOT SLO budgets
  - attention score distribution (which layers/heads are sparse)
  - block size in bytes
  - inter-node bandwidth topology
  - recompute cost relative to fetch cost per tier
owns: ['admission', 'eviction', 'promotion', 'demotion', 'placement', 'GC']
excludes:
  - block value computation (see cache-value-and-recompute)
  - block geometry and sizing (see kv-footprint-and-lifecycle)
  - IO pipeline depth and concurrency (see data-movement-concurrency)
  - distributed ownership and routing (see distributed-kv-ownership)
  - hardware specifications
  - attention kernel internals
related:
  - cache-value-and-recompute
  - kv-footprint-and-lifecycle
  - data-movement-concurrency
  - distributed-kv-ownership
  - bottleneck-and-measurement
  - workload-to-storage-io
---

## 1. Decision Being Made

The optimization agent must decide, for every KV block in the system:

1. **Admission**: Should a newly computed block be admitted to persistent cache, or is it too unlikely to be reused? Admission filtering prevents cache pollution from one-shot blocks.
2. **Placement**: Given multiple tiers (GPU HBM → host DRAM → NVMe SSD → remote pool), which tier should the block initially land in? This is a bandwidth-latency-capacity tradeoff.
3. **Promotion**: When a block in a lower tier receives a hit or is predicted to be needed soon, should it move to a faster tier? Promotion has transfer cost and may trigger eviction elsewhere.
4. **Demotion**: When a higher tier is under pressure, which blocks should be pushed down rather than evicted entirely? Demotion preserves the block's recompute savings at higher access latency.
5. **Eviction**: When all tiers are full or a block's value drops below the storage cost threshold, which block should be permanently discarded?
6. **GC**: When a block's reference count reaches zero (all dependent sequences have completed or been aborted), how aggressively should the block be reclaimed vs speculatively retained?

These decisions must be made at the timescale of token generation steps (milliseconds), not offline. The wrong choice manifests as either SLO violations (evict too aggressively → recompute stalls prefill) or memory exhaustion (admit too freely → batch size drops → throughput loss).


## 2. Mental Model and Equations

### 2.1 Eviction Priority Score

The canonical priority for evicting block `b` at time `t`:

```
priority_evict(b) = V(b) / size(b)
```

Where `V(b) = P_reuse(b) × C_saved(b) − C_store(b, Δt)` from cache-value-and-recompute. Evict the block with the **lowest** priority. This is the GDSF (Greedy Dual Size Frequency) principle: size-normalized value determines eviction order.

### 2.2 Admission Filter (TinyLFU Principle)

Admit candidate block `c` only if:

```
admit(c) = freq_estimate(c) > freq_estimate(victim)
```

Where `victim` is the current eviction candidate. A count-min sketch provides `freq_estimate` with sub-linear memory. This prevents newly-seen blocks from displacing proven-valuable residents—critical for workloads mixing multi-turn conversations (high reuse) with one-shot completions (zero reuse).

### 2.3 Tier Assignment (Cost-Bandwidth Model)

Optimal tier for block `b` expected to be accessed within interval `Δt`:

```
tier*(b) = argmin_t [ C_store(b, t, Δt) + P_access(b, Δt) × latency(t) × SLO_penalty ]
```

Where:
- `C_store(b, t, Δt)` = cost of holding block in tier `t` for expected interval
- `latency(t)` = access latency of tier `t` (HBM ~1μs, DRAM ~10μs, SSD ~100μs, remote ~1ms)
- `SLO_penalty` = marginal cost of latency against TTFT or TPOT budgets

### 2.4 Promotion Trigger

Promote block `b` from tier `t_low` to `t_high` when:

```
P_access(b, window) × (latency(t_low) − latency(t_high)) > C_transfer(b, t_low→t_high) / amortization_factor
```

The amortization factor accounts for how many future accesses justify the one-time transfer cost. Set too low → thrashing; too high → cold blocks stay in slow tiers.

### 2.5 Demotion vs Eviction Threshold

Demote rather than evict when:

```
V(b) > C_transfer(b, t_high→t_low) + C_store(b, t_low, expected_residency)
```

Otherwise evict outright—the block is not worth the transfer cost to a lower tier.

### 2.6 GC Aggressiveness

For zero-reference blocks, the speculative retention criterion:

```
retain(b) = P_reuse_future(b) × C_recompute(b) > C_store(b, t, TTL_speculative)
```

Where `P_reuse_future` is estimated from the prefix frequency distribution. Shared system-prompt prefixes have `P_reuse_future ≈ 1.0`; unique conversation suffixes approach `0.0` post-completion.


## 3. Required Observations

Before making tier/eviction decisions, the agent must measure:

| Observation | Why Needed | Source |
|---|---|---|
| Per-tier utilization (%) | Determines urgency of eviction/demotion | Memory allocator stats |
| Block access frequency (per block or prefix class) | Drives eviction priority and admission | Count-min sketch or radix tree counters |
| Time since last access per block | Recency signal for LRU-family policies | Block metadata timestamps |
| TTFT and TPOT percentiles vs SLO | Determines if eviction is causing recompute stalls | Request latency histogram |
| Recompute rate (blocks/sec being regenerated) | High rate signals over-aggressive eviction | Prefill scheduler metrics |
| Inter-tier bandwidth utilization | Bottleneck detection for promotion/demotion | IO counters |
| Prefix distribution entropy | Low entropy → more sharing → admission bias toward prefixes | Prefix tree statistics |
| Batch size relative to maximum | If batch limited by memory, eviction increases throughput | Scheduler state |
| Cache hit rate by tier | Validates policy effectiveness | Hit/miss counters |
| Block temperature distribution | Bimodal → tiering helps; uniform → simpler policies suffice | Access pattern histogram |


## 4. Alternatives with Prefer/Avoid

### 4.1 LRU (Least Recently Used)

- **Mechanism**: Evict the block whose last access is oldest. O(1) with doubly-linked list + hash map.
- **Prefer when**: Access patterns are strongly recency-correlated (multi-turn chat with temporal locality); implementation simplicity is valued; workload is homogeneous.
- **Avoid when**: Workload has scan patterns (one-shot requests that touch many blocks once, polluting the recency list); block sizes vary significantly; frequency matters more than recency (shared system prompts accessed infrequently but reliably).

### 4.2 LFU (Least Frequently Used)

- **Mechanism**: Evict the block with the lowest access count. Requires frequency counters per block.
- **Prefer when**: Workload has a stable hot set (common system prompts, RAG retrieval prefixes); frequency is a better predictor than recency.
- **Avoid when**: Access patterns are non-stationary (workload shifts → stale frequency counts block new entries); cold-start problem for new blocks; scan resistance is needed.

### 4.3 ARC (Adaptive Replacement Cache)

- **Mechanism**: Maintains two LRU lists (L1 for recency, L2 for frequency) plus ghost entries. Dynamically adjusts the partition between them based on which ghost list gets hit. Self-tuning between LRU and LFU behavior. (Megiddo & Modha, FAST 2003)
- **Prefer when**: Workload pattern is unknown or changes over time; you need scan resistance without manual tuning; mixed traffic (multi-turn + one-shot).
- **Avoid when**: Memory overhead for ghost lists is prohibitive (each ghost entry costs metadata space); block count is enormous (millions of fine-grained blocks); simpler policies already achieve >95% hit rate.

### 4.4 TinyLFU + W-TinyLFU (Window-TinyLFU)

- **Mechanism**: Admission filter based on count-min sketch frequency estimator. Window variant: small LRU window for admission, SLRU main cache, TinyLFU gate between them. (Einziger et al., 2017)
- **Prefer when**: Cache pollution from one-shot requests is a primary concern; memory for metadata must be minimal (sketch is sub-linear); workload has heavy-tailed popularity distribution.
- **Avoid when**: All blocks have similar access frequency (filter provides no discrimination); block access patterns are bursty with long gaps (sketch aging may drop valid entries).

### 4.5 GDSF (Greedy Dual Size Frequency)

- **Mechanism**: Priority = (frequency × cost) / size. Evict lowest priority. Generalizes LFU by accounting for heterogeneous block sizes and retrieval costs.
- **Prefer when**: Block sizes vary significantly (partial blocks, compressed blocks, variable-length prefixes); different blocks have different recompute costs; economic value model is available.
- **Avoid when**: All blocks are uniform size and cost (collapses to LFU, adds unnecessary complexity); priority computation cost is problematic at very high eviction rates.

### 4.6 Prefix-Aware / Radix Tree Eviction (SGLang-style)

- **Mechanism**: Organize blocks as nodes in a radix tree keyed by token prefix. Evict leaf-to-root: remove the least valuable leaf, then its parent only if it becomes a leaf. Preserves shared prefix structure. (SGLang RadixAttention)
- **Prefer when**: Workload has high prefix sharing (multi-turn, RAG with common retrievals, batched requests with system prompts); you need structural coherence in eviction.
- **Avoid when**: Prefix sharing is minimal (unique long-context requests); tree maintenance overhead exceeds benefit; workload is decode-dominated with little prefix reuse.

### 4.7 Predictive / Speculative Eviction

- **Mechanism**: Use attention patterns or scheduler queue state to predict which blocks will be needed in the near future. Proactively promote predicted-hot blocks and evict predicted-cold ones before demand arrives. (InfiniGen-style speculative rehearsal)
- **Prefer when**: Offloaded tiers have high latency (100μs+ for SSD), making reactive fetch too slow; attention patterns are predictable from partial computation; prefetch bandwidth is available.
- **Avoid when**: Prediction accuracy is low (unpredictable attention patterns); prefetch bandwidth competes with active serving traffic; the overhead of running the predictor exceeds the benefit of early movement.

### 4.8 Tiered Demotion Cascade

- **Mechanism**: Never evict outright from any tier except the last. Each tier demotion decision is independent: GPU→DRAM on GPU pressure, DRAM→SSD on DRAM pressure, SSD→discard on SSD pressure. (FlexGen/llm-d hierarchy model)
- **Prefer when**: Recompute cost is high relative to transfer cost; multiple tiers have sufficient bandwidth; SSD capacity is large and access latency is tolerable for some traffic.
- **Avoid when**: Lower tiers have bandwidth insufficient to serve at the required rate; transfer cost exceeds recompute cost (small blocks, fast GPUs); system is latency-critical with no SLO headroom for slow-tier access.


## 5. Coupled Constraints

### 5.1 Eviction ↔ Batch Size
Evicting blocks frees memory for new requests (increases batch size and throughput). But if evicted blocks are soon needed, recompute cost may exceed the throughput gained. The equilibrium: evict until marginal recompute cost equals marginal throughput gain from the freed memory.

### 5.2 Admission ↔ Cache Hit Rate
Strict admission filters (TinyLFU) improve hit rate for admitted blocks but reduce total cache coverage. Under low-reuse workloads, a strict filter effectively disables caching. Under high-reuse workloads, it eliminates pollution. The filter's aggression must track workload reuse statistics.

### 5.3 Promotion ↔ Bandwidth Budget
Every promotion (SSD→DRAM or DRAM→GPU) consumes inter-tier bandwidth that could serve active decode traffic. Over-promotion starves serving; under-promotion causes SLO violations from slow-tier access. A bandwidth reservation (e.g., 20% for migration, 80% for serving) prevents interference.

### 5.4 Placement ↔ Prefix Sharing
Placing all copies of a popular prefix on one node maximizes that node's hit rate but creates a hotspot. Distributing copies across nodes improves load balance but wastes aggregate storage. Replication factor should track access frequency: `replicas = min(N, ceil(access_rate / per_node_serve_capacity))`.

### 5.5 GC Aggressiveness ↔ Cold-Start Latency
Aggressive GC (reclaim immediately on refcount=0) maximizes available capacity but causes cold starts when the same prefix is requested again soon. Lazy GC (retain speculatively) smooths cold starts but holds dead memory. The optimal TTL for speculative retention depends on inter-arrival time of matching prefixes.

### 5.6 Eviction ↔ Compression
CacheGen demonstrated 3.5–4.3× KV cache compression (SIGCOMM 2024). Compressing before demotion effectively multiplies lower-tier capacity but adds encode/decode latency. The decision: compress-and-demote vs evict-and-recompute depends on whether `decode_latency + access_latency(lower_tier) < recompute_latency`.


## 6. Failure Modes

### 6.1 Thrashing
**Trigger**: Working set exceeds tier capacity by a small margin. Blocks are continuously evicted and re-fetched. **Symptom**: High promotion/demotion rate with no improvement in hit rate. **Diagnostic**: If block-movement bandwidth utilization > 50% and hit rate < 80%, thrashing is likely.

### 6.2 Cache Pollution
**Trigger**: One-shot requests admitted without filtering. They evict high-value shared prefixes that subsequently cause expensive recompute. **Symptom**: Hit rate drops during traffic spikes of new unique prompts. **Diagnostic**: Access frequency of evicted blocks was significantly higher than that of admitted blocks.

### 6.3 Priority Inversion
**Trigger**: A low-value decode-phase block in GPU HBM prevents admission of a high-value prefill block that would save 10× more compute. **Symptom**: TTFT SLO violations co-occurring with under-utilized GPU KV cache pages. **Diagnostic**: Eviction priority score of oldest resident < admission score of rejected candidate.

### 6.4 Stale Residency (GC Failure)
**Trigger**: Blocks whose owning requests completed remain in high-value tiers because GC is too lazy. Accumulated dead blocks reduce effective capacity. **Symptom**: Reported capacity utilization is high but hit rate is declining. **Diagnostic**: Fraction of blocks with zero references and zero hits in last N seconds exceeds 30%.

### 6.5 Bandwidth Starvation from Over-Promotion
**Trigger**: Aggressive speculative promotion saturates inter-tier links, delaying active serving reads. **Symptom**: Decode TPOT increases; inter-tier bandwidth is fully utilized but serving-path read latency spikes. **Diagnostic**: Serving-read queue depth increases while migration traffic dominates the link.

### 6.6 Cold-Tier Amplification
**Trigger**: SSD tier fills with blocks that are never re-accessed, consuming write bandwidth and wearing flash. DRAM pressure remains high because "SSD is available" biases toward demotion over eviction. **Symptom**: SSD write amplification increases; blocks demoted to SSD are eventually discarded without a single re-read. **Diagnostic**: Ratio of SSD evictions to SSD hits > 10:1.


## 7. Hypotheses the Agent Can Generate

1. **H1**: "Enabling TinyLFU admission filtering will increase effective hit rate by >15% under mixed one-shot/multi-turn traffic without reducing throughput." (Testable: A/B on admission filter on/off.)

2. **H2**: "Switching eviction policy from pure LRU to GDSF (value/size priority) will reduce recompute rate by >30% because large shared-prefix blocks have high value-per-byte." (Testable: Measure recompute rate before/after.)

3. **H3**: "Demoting blocks to SSD (rather than evicting) will improve TTFT p99 by >20% when recompute cost exceeds SSD fetch cost." (Testable: Compare TTFT with demotion-enabled vs evict-only under the same workload.)

4. **H4**: "The current GC TTL is too aggressive: >25% of recomputed blocks were GC'd less than 60s before re-access." (Testable: Log time-since-GC for every recomputed block.)

5. **H5**: "Prefix-aware radix eviction will outperform flat LRU by >2× hit rate when prefix sharing ratio exceeds 40% of active blocks." (Testable: Measure hit rate under both policies with instrumented prefix sharing counter.)

6. **H6**: "Reserving 20% of inter-tier bandwidth for serving (vs allowing 100% migration) will improve TPOT p99 by >15% without measurably reducing hit rate." (Testable: Compare with/without bandwidth reservation under load.)

7. **H7**: "Compressing blocks 4× before SSD demotion (CacheGen-style) increases effective SSD capacity enough to reduce eviction rate by >50%." (Testable: Measure eviction rate and decode overhead with compression enabled.)


## 8. Experiments and Falsifiers

### E1: Admission Filter Impact
- **Setup**: Run identical workload with and without TinyLFU admission gate.
- **Metric**: Hit rate, recompute rate, throughput.
- **Falsifier for H1**: If hit rate does not increase by >15%, or if throughput drops (cold-start penalty from rejected blocks exceeds pollution savings), H1 is falsified.

### E2: Eviction Policy Comparison
- **Setup**: Hold workload constant; switch between LRU, GDSF, ARC. Measure over 30-minute windows.
- **Metric**: Recompute rate, SLO attainment, memory waste.
- **Falsifier for H2**: If recompute rate difference between GDSF and LRU is <10%, the value model does not discriminate well enough under this workload.

### E3: Demotion Benefit
- **Setup**: Enable SSD tier for demotion vs evict-only baseline under memory pressure.
- **Metric**: TTFT p99, SSD read/write bandwidth utilization, overall throughput.
- **Falsifier for H3**: If TTFT p99 does not improve (SSD latency too high or bandwidth saturated), demotion adds cost without benefit.

### E4: GC Retention Analysis
- **Setup**: Instrument GC to log `(block_id, time_GC'd, time_next_access_if_any)`.
- **Metric**: Fraction of GC'd blocks re-requested within 60s, 300s, 600s.
- **Falsifier for H4**: If <5% of GC'd blocks are re-requested within 60s, GC is not the cause of recomputation; look elsewhere.

### E5: Prefix-Aware vs Flat Eviction
- **Setup**: Same workload, same capacity. Compare radix-tree eviction (leaf-first) vs flat LRU.
- **Metric**: Hit rate, prefix integrity (fraction of complete prefixes in cache).
- **Falsifier for H5**: If prefix sharing is <20% of blocks or hit rate difference is <10%, structural eviction overhead is not justified.

### E6: Bandwidth Reservation
- **Setup**: Set migration bandwidth cap at 20%, 50%, 80%. Measure under sustained load.
- **Metric**: TPOT p99, hit rate, migration queue depth.
- **Falsifier for H6**: If TPOT p99 does not correlate with migration traffic (migration is not the bottleneck), the reservation is unnecessary overhead.

### E7: Compression Before Demotion
- **Setup**: Demote blocks raw vs 4× compressed. Measure effective capacity, decode latency overhead, eviction rate.
- **Falsifier for H7**: If decode overhead (decompression) pushes TPOT beyond SLO, compression is net-negative despite capacity gains.


## 9. Production Evidence

### Mooncake (Kimi, FAST 2025)
- **Problem**: Kimi's long-context workloads created KV caches far exceeding GPU memory; one-shot requests polluted shared prefix cache.
- **Mechanism**: KVCache-centric disaggregated architecture with DRAM+SSD tier pool and prediction-based early rejection (admission control). Scheduler balances throughput against SLO.
- **Result**: 525% throughput increase in simulated overload scenarios; 75% more requests handled under production traffic at Kimi with SLO adherence.
- **Lesson**: Admission control (early rejection) is as important as eviction. Refusing to admit work you cannot serve within SLO is itself a tier policy decision.

### SGLang RadixAttention (Zheng et al., 2024)
- **Problem**: Multi-turn and few-shot workloads share long token prefixes; flat LRU evicts shared prefixes after the last request in a burst, causing recompute on next burst.
- **Mechanism**: Radix tree indexes blocks by token prefix; LRU eviction respects tree structure (leaf-first). Shared interior nodes are preserved until all children are evicted.
- **Result**: 6.4× higher throughput versus prior systems across agent, RAG, and multi-turn chat workloads.
- **Lesson**: Prefix-aware eviction structure preserves high-value shared blocks that flat policies would destroy. The data structure IS the policy.

### vLLM PagedAttention (Kwon et al., SOSP 2023)
- **Problem**: Contiguous KV allocation causes 60–80% memory waste from fragmentation, limiting batch size and throughput.
- **Mechanism**: Block-level paging with all-or-nothing eviction per sequence. Preemption swaps entire sequences to CPU when GPU is full, allowing other requests to proceed.
- **Result**: 2–4× throughput improvement over FasterTransformer/Orca at same latency; improvement grows with longer sequences and larger models.
- **Lesson**: Granular block management (not just policy choice) determines effective capacity. Even simple eviction (all-or-nothing per sequence) works well when fragmentation is eliminated.

### FlexGen (Sheng et al., ICML 2023)
- **Problem**: Running OPT-175B on a single 16GB GPU requires offloading KV cache to CPU DRAM and SSD.
- **Mechanism**: Linear programming determines optimal block placement across GPU/CPU/SSD. Zig-zag scheduling minimizes redundant transfers. 4-bit quantization expands effective capacity.
- **Result**: First system to achieve 1 token/s generation for OPT-175B on single 16GB GPU (effective batch 144). Throughput-optimal placement found automatically.
- **Lesson**: Tier placement is a constrained optimization problem amenable to LP when the cost model (bandwidth × latency × capacity) is known. Automated placement beats hand-tuned heuristics.

### InfiniGen (Lee et al., OSDI 2024)
- **Problem**: Offloaded KV in host DRAM incurs high fetch latency if entire cache must be retrieved; selective retrieval needed.
- **Mechanism**: Speculative rehearsal using current layer inputs + partial next-layer weights to predict high-attention tokens. Only essential KV entries are prefetched from host DRAM to GPU.
- **Result**: 3× performance improvement over prior offloading methods with better model accuracy (no approximation of attention).
- **Lesson**: Not all blocks in a tier need to be treated equally. Selective promotion based on predicted access patterns (attention scores) converts a tier boundary from a latency wall into a managed cache.

### llm-d (CNCF, 2024–2025)
- **Problem**: Multi-node inference clusters waste capacity when KV cache is per-instance; requests routed to wrong nodes cause recompute.
- **Mechanism**: Global KV cache index + prefix-cache-aware routing. Hierarchical tier offload (GPU→CPU→disk) with 13.9× throughput gain at 250 concurrent users vs GPU-only. Prefix-aware routing gives 3× throughput vs round-robin.
- **Result**: 70% higher tokens/sec from disaggregated prefill/decode; 40% TTFT/ITL reduction from predicted-latency-based placement.
- **Lesson**: Placement across nodes is eviction's complement. Router-level cache awareness eliminates most "evict then recompute elsewhere" patterns.

### CacheGen (Liu et al., SIGCOMM 2024)
- **Problem**: Transferring raw FP16 KV cache between prefill and decode nodes saturates network; limits disaggregation benefit.
- **Mechanism**: Distribution-aware tensor encoding compresses KV to 3.5–4.3× smaller bitstream. Adaptive compression trades quality margin for bandwidth under congestion.
- **Result**: 3.2–3.7× reduction in fetch+process delay with negligible quality loss.
- **Lesson**: Compression is a tier-policy multiplier—it changes the effective capacity and bandwidth of every tier, shifting all eviction/demotion thresholds. The compression ratio should be a variable in the tier assignment equation, not a fixed assumption.

### DistServe (Zhong et al., OSDI 2024)
- **Problem**: Co-located prefill and decode interfere; eviction pressure from one phase disrupts the other's SLO.
- **Mechanism**: Physical disaggregation of prefill/decode to separate GPU pools. KV placed on decode nodes based on bandwidth topology. Co-optimized resource allocation per phase.
- **Result**: 7.4× more requests served within SLO or 12.6× tighter SLO bounds vs co-located systems.
- **Lesson**: Tier policy must be phase-aware. Prefill-generated blocks have different lifetime profiles (write-once, read-many for long decode) than decode-extended blocks (write-many, read-once per step). Separate pools avoid interference-driven eviction.


## 10. Implications for KV Block Storage

1. **Multi-tier is mandatory, not optional**. Production evidence uniformly shows that single-tier GPU-only cache leaves 2–10× throughput on the table. A block storage system must natively support GPU HBM, host DRAM, and NVMe SSD tiers with explicit placement APIs.

2. **Admission control before write**. The storage layer should expose an admission gate that the policy layer can invoke before committing a write. This is not a "nice-to-have filter"—Mooncake's 525% gain relies on rejecting work that would pollute cache.

3. **Block-level metadata is cheap; use it**. Each block needs: access count, last access timestamp, reference count, prefix hash, tier location, compression state. This metadata (<64 bytes per block) enables GDSF, TinyLFU, and prefix-aware eviction. The storage layer should maintain it atomically with block operations.

4. **Demotion path must be as fast as eviction**. If demoting to SSD takes longer than recomputing the block, the demotion tier is useless. The storage layer must provide async demotion that does not block the eviction critical path. Write-behind with eviction-on-pressure is the pattern.

5. **Compression is a storage-layer concern**. CacheGen's 3.5–4.3× compression changes effective tier capacity. The storage layer—not the serving engine—should own encode/decode, since it can amortize across all clients and adjust compression level per tier and per block value.

6. **Prefix structure must be visible to eviction**. Radix tree eviction (SGLang 6.4× gain) requires the storage layer to understand block-to-prefix relationships. Exposing a `parent` pointer or tree-walk API enables prefix-aware policies without embedding tree logic in the serving engine.

7. **Global placement index for multi-node**. llm-d's 3× gain from prefix-aware routing requires a global index mapping prefix hashes to node locations. The storage system is the natural owner of this index, since it already tracks block locations for local eviction.

8. **Phase-aware lifetime hints**. Blocks born in prefill (write-once, read-many during decode) differ from blocks extended during decode (append-only, read-once per step). The storage layer should accept lifetime-class hints at write time, enabling separate eviction queues per class without the serving engine managing multiple policy instances.

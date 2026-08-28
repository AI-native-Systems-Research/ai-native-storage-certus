I now have enough context and evidence from the research. Let me produce the full domain reference file.

---

```markdown
---
id: distributed-kv-ownership
decision: Which node or service should own, store, and serve KV blocks in a disaggregated multi-node inference cluster
answers:
  - Who should own or serve KV in a disaggregated system?
  - How should KV blocks be placed across nodes to maximize reuse and minimize transfer?
  - When should KV be replicated vs single-homed?
  - How do nodes discover and route to cached KV blocks?
  - What happens to in-flight decodes when a KV-owning node fails?
  - How is coherence maintained when multiple writers produce the same prefix?
inputs:
  - cluster topology and inter-node bandwidth
  - prefix popularity distribution (Zipf exponent)
  - request arrival pattern and prefix locality
  - per-node GPU memory capacity and utilization
  - TTFT and TPOT SLO budgets
  - KV block size and transfer latency
  - replication budget (storage cost multiplier)
  - failure rate and detection latency of nodes
owns: ['placement', 'routing', 'replication', 'coherence', 'failover']
excludes:
  - per-node tier policy and eviction (see tier-policy-and-eviction)
  - block value computation (see cache-value-and-recompute)
  - IO pipeline depth within a node (see data-movement-concurrency)
  - attention kernel internals
  - hardware specifications
related:
  - tier-policy-and-eviction
  - cache-value-and-recompute
  - data-movement-concurrency
  - kv-footprint-and-lifecycle
  - workload-to-storage-io
  - bottleneck-and-measurement
---

## 1. Decision Being Made

The optimization agent must decide, for every KV block in a multi-node cluster:

1. **Placement**: Which node(s) should hold a newly computed KV block? This determines whether subsequent requests for the same prefix find a local hit or pay transfer latency.
2. **Routing**: Given a request whose prefix partially matches cached KV on various nodes, which node should serve it? Route-to-cache maximizes reuse; route-to-compute minimizes queuing delay.
3. **Replication**: Should popular prefix blocks be replicated across multiple nodes? Replication eliminates hotspots but costs memory proportional to the replication factor.
4. **Coherence**: When the same logical prefix is computed independently by multiple nodes (or when a block is updated by continued generation), which copy is authoritative? Stale KV produces wrong attention outputs—not just inefficiency but correctness failures.
5. **Failover**: When a node holding KV blocks becomes unreachable, should dependent decode requests recompute from scratch, redirect to a replica, or wait for recovery?

These decisions operate at request-scheduling timescale (tens of milliseconds). Wrong placement wastes inter-node bandwidth. Wrong routing causes prefix cache misses that trigger full recompute (hundreds of milliseconds for long prefixes). Wrong replication either creates memory pressure (over-replicated) or hotspot-induced queuing (under-replicated). Incoherent KV silently corrupts output quality.


## 2. Mental Model and Equations

### 2.1 Placement Utility

For a block `b` with prefix hash `h`, the utility of placing it on node `n`:

```
U_place(b, n) = P_hit(h, n) × C_saved(b) − C_store(b, n) − C_transfer(b, n)
```

Where:
- `P_hit(h, n)` = probability a future request routed to `n` needs prefix `h`
- `C_saved(b)` = compute cost of regenerating block `b` from scratch (proportional to sequence length × model FLOP/token)
- `C_store(b, n)` = opportunity cost of memory consumed on node `n` (displaces batch slots)
- `C_transfer(b, n)` = cost of moving the block to `n` if generated elsewhere

Optimal placement maximizes `Σ U_place(b, n)` across all blocks and nodes.

### 2.2 Routing Decision (Prefix-Aware)

For request `r` with prefix tokens `T_r`, route to node:

```
n* = argmax_n [ match_length(T_r, cache(n)) × value_per_token − queue_delay(n) × SLO_penalty ]
```

Where:
- `match_length(T_r, cache(n))` = longest prefix match available on node `n`
- `value_per_token` = recompute cost avoided per cached token (FLOP_prefill / token)
- `queue_delay(n)` = estimated wait time on node `n`'s queue
- `SLO_penalty` = marginal cost of additional latency against TTFT budget

This is the core tradeoff: route to a busy node with a cache hit, or to an idle node requiring recompute?

### 2.3 Replication Factor

For prefix `h` with access rate `λ_h` and per-node serve capacity `μ`:

```
R(h) = min(N, ceil(λ_h / μ))
```

Where:
- `N` = total nodes in cluster
- `λ_h` = requests/sec needing prefix `h`
- `μ` = max requests/sec a single node can serve for decode (limited by memory bandwidth)

Replicate only when a single node cannot absorb the demand. Over-replication wastes memory that could hold other blocks; under-replication creates queuing hotspots.

### 2.4 Coherence Window

A KV block becomes stale when the model state it represents diverges from the authoritative computation. The coherence requirement:

```
valid(b, t) = (t − t_computed(b)) < TTL(b) ∧ ¬invalidated(b)
```

For prefix blocks (immutable once sealed), TTL is infinite—they are content-addressed and immutable. For decode-phase blocks (appended per token), coherence is per-sequence: only the generating node's copy is valid. This asymmetry is fundamental: prefixes are safely shared; suffixes are not.

### 2.5 Transfer Break-Even

Transferring block `b` from node `src` to node `dst` is worthwhile when:

```
C_recompute(b) > C_transfer(b) = size(b) / bandwidth(src, dst) + overhead_fixed
```

For a 70B model with 128 layers, a 4K-token prefix block ≈ 2–4 GB. At 100 Gbps inter-node bandwidth, transfer takes 160–320 ms. Recompute at the same length on an H100 takes ~200–800 ms depending on batch. The crossover point depends heavily on prefix length and available compute.

### 2.6 Consistent Hashing for Placement

Map prefix hashes to nodes via consistent hashing (Karger et al., 1997):

```
node(h) = successor(hash(h), ring)
```

With `V` virtual nodes per physical node, rebalancing on node addition/removal affects only `1/N` of keys. This provides O(1) lookup and minimal disruption, the same principle underlying Amazon Dynamo. The ring maps prefix content hashes to responsible nodes.


## 3. Required Observations

Before making placement/routing decisions, the agent must measure:

| Observation | Why Needed | Source |
|---|---|---|
| Per-node prefix hit rate | Validates current placement effectiveness | Cache hit counters per node |
| Inter-node transfer rate (blocks/sec) | High rate signals placement misalignment | Network IO counters |
| Prefix popularity distribution | Determines replication need (Zipf α) | Request log prefix histogram |
| Per-node queue depth and latency | Routing must balance cache hits vs load | Scheduler queue metrics |
| Cross-node bandwidth utilization | Capacity constraint for transfers | Network interface stats |
| KV block locality (% requests served locally) | Target metric for placement quality | Routing decision log |
| Recompute events caused by remote misses | Direct cost of suboptimal placement | Prefill scheduler counters |
| Node failure frequency and detection time | Sizes the failover budget | Health check logs |
| Prefix sharing ratio across concurrent requests | Determines value of hot-prefix replication | Prefix tree overlap analysis |
| Transfer latency percentiles (p50, p99) | Validates whether transfer beats recompute | Transfer completion timestamps |
| Coherence violations detected | Signals staleness in replicated blocks | Version-check counters |


## 4. Alternatives with Prefer/Avoid

### 4.1 Prefix-Hash Consistent Hashing (Dynamo-style)

- **Mechanism**: Hash the prefix content (token sequence hash) onto a consistent hash ring. Each prefix has a deterministic home node. Requests with that prefix route to the home. Virtual nodes balance load across physical nodes. (Karger et al., 1997; DeCandia et al., SOSP 2007)
- **Prefer when**: Prefix distribution is relatively uniform; cluster membership is stable; prefix blocks are large enough that the routing overhead is negligible relative to transfer savings; system prioritizes deterministic, stateless routing decisions.
- **Avoid when**: Prefix popularity is extremely skewed (Zipf α > 1.5)—hot prefixes overwhelm their home node; frequent node churn causes excessive rebalancing; the system needs to route based on both prefix match AND compute availability simultaneously.

### 4.2 Centralized Global KV Index (llm-d / Mooncake-style)

- **Mechanism**: A central metadata service (or distributed index like Redis) tracks which nodes hold which prefix blocks. The router queries the index, finds the node with the longest prefix match, and routes there. The index is updated on every admission and eviction. (llm-d "precise global indexing"; Mooncake KVCache-centric scheduler)
- **Prefer when**: Prefix hit lookup must consider partial matches (not just exact hashes); routing decisions must jointly optimize cache hit and load balance; cluster is large enough that broadcast-based discovery is infeasible; prefix blocks move between nodes (tiered offloading).
- **Avoid when**: Index becomes a bottleneck or single point of failure at very high request rates (>100K req/s); index update lag creates stale routing (request arrives before eviction is indexed); operational complexity of maintaining a distributed index exceeds the benefit for small clusters (<8 nodes).

### 4.3 P2P Gossip-Based Discovery

- **Mechanism**: Nodes periodically advertise their cached prefix hashes to neighbors. Routers maintain an approximate, eventually-consistent view of cluster-wide cache state. No central index required. (Inspired by distributed hash tables / Chord)
- **Prefer when**: Cluster is too large or too dynamic for a centralized index; eventual consistency is acceptable (prefix blocks are stable for seconds+); network partition tolerance is required; operational simplicity is valued over routing precision.
- **Avoid when**: Prefix set changes rapidly (high eviction/admission rate invalidates gossip before it propagates); routing precision is critical (stale gossip → cache misses → wasted recompute); cluster is small enough that a centralized index is trivially reliable.

### 4.4 Affinity-Based Sticky Routing

- **Mechanism**: Route based on session or user affinity—all requests from the same session go to the same node, maximizing multi-turn KV reuse without any global coordination. Simple hash of session ID to node. (SGLang RadixAttention implicit locality)
- **Prefer when**: Workload is dominated by multi-turn conversations (high intra-session prefix reuse); session-to-node mapping is stable; sessions have similar compute requirements (load remains balanced); no need to share prefixes across sessions.
- **Avoid when**: Many sessions share the same system prompt (affinity prevents cross-session prefix sharing); sessions vary wildly in length/compute (creates load imbalance); sessions are short-lived (affinity provides no benefit if sessions are one-shot); node failures require re-homing all affected sessions.

### 4.5 Disaggregated Prefill-Decode Split (DistServe/Splitwise-style)

- **Mechanism**: Prefill nodes compute KV and transfer it to dedicated decode nodes. Ownership is phase-based: prefill nodes own blocks during computation; decode nodes own them during generation. Transfer uses RDMA/NVLink between dedicated pools. (DistServe, OSDI 2024; Splitwise, ISCA 2024)
- **Prefer when**: Prefill and decode have fundamentally different hardware requirements (compute-bound vs memory-bandwidth-bound); cluster can afford dedicated pools; transfer bandwidth (NVLink/RDMA) is sufficient to move KV within TTFT budget; request rate justifies the infrastructure cost.
- **Avoid when**: KV transfer latency exceeds recompute time (short prefixes, slow interconnect); cluster is too small to dedicate hardware to each phase; workload is decode-dominated (prefill pool sits idle); KV blocks are too large relative to available bandwidth (70B model, 100K+ context).

### 4.6 Hierarchical Tiered Ownership (LMCache/Mooncake KVPool)

- **Mechanism**: Each node owns its GPU-tier KV locally. A shared pool (CPU DRAM, remote object store, or dedicated KV server) holds demoted blocks accessible to any node. Ownership transitions: GPU-local → shared pool (on eviction) → any node (on hit). The shared pool acts as a cluster-wide L2 cache. (LMCache multi-node P2P sharing; Mooncake distributed KVCache pool)
- **Prefer when**: GPU memory is insufficient to hold working set on any single node; inter-node bandwidth supports shared-pool fetch within SLO; prefix reuse spans multiple nodes (shared system prompts); you need flexible failover (shared pool survives node failures).
- **Avoid when**: Shared pool access latency violates TTFT SLO; shared pool becomes a bandwidth bottleneck under high fan-in; KV blocks are accessed so frequently that the extra hop (node → pool → node) adds unacceptable overhead compared to local GPU access.


## 5. Coupled Constraints

### 5.1 Placement ↔ Routing Consistency
Placement decides where blocks live; routing decides where requests go. If placement is prefix-hash-based but routing ignores the hash (e.g., round-robin), every request misses the cache. The two policies must be co-designed: either route-to-cache (routing follows placement) or place-to-route (placement follows where traffic naturally lands).

### 5.2 Replication ↔ Eviction Pressure
Replicating a popular prefix to R nodes consumes R× memory. Under memory pressure, eviction must coordinate across replicas—if all replicas independently evict the same block, the cluster loses it entirely. Replication increases aggregate storage cost and tightens eviction budgets on every participating node.

### 5.3 Transfer Bandwidth ↔ Serving Bandwidth
KV block transfers between nodes share the network fabric with decode-serving traffic (attention value reads for remote KV access). Saturating the network with placement/migration traffic starves active decodes. A bandwidth reservation (e.g., llm-d's separation of migration vs serving lanes) prevents interference.

### 5.4 Coherence ↔ Replication Lag
Replicating a block to multiple nodes introduces a consistency window. If a block is invalidated on the primary (e.g., associated session aborts) but replicas are not yet notified, stale replicas may serve incorrect KV. For immutable prefix blocks this is benign (content-addressed → no invalidation), but for session-specific suffix blocks it causes correctness errors.

### 5.5 Failover ↔ TTFT Budget
On node failure, recomputing lost KV blocks from scratch takes O(prefix_length) time. If the TTFT budget is tight, failover must redirect to a replica (if one exists) rather than recompute. This couples failover strategy to replication policy: tighter SLOs demand higher replication factors for critical prefixes.

### 5.6 Placement ↔ Compression
CacheGen compresses KV 3.5–4.3× (SIGCOMM 2024). Compressed blocks can be stored on remote nodes at reduced bandwidth cost, making remote placement viable where raw KV transfer would be too slow. Compression changes the break-even calculation: `C_transfer(b, compressed) = size(b) / compression_ratio / bandwidth`, shifting more blocks into "worth transferring" territory.


## 6. Failure Modes

### 6.1 Hot-Prefix Hotspot
**Trigger**: A dominant system prompt (used by >50% of requests) is placed on a single node via consistent hashing. That node is overwhelmed while others are idle. **Symptom**: One node at 95%+ utilization, others at <40%; TTFT skewed high for affected prefix. **Diagnostic**: Standard deviation of per-node hit rate exceeds 30% of mean.

### 6.2 Stale Index Routing
**Trigger**: Global KV index has update lag (evictions not yet propagated). Router sends request to node that already evicted the prefix. Request suffers cache miss AND queuing delay on an overloaded node. **Symptom**: Apparent hit rate from router perspective >> actual hit rate at node. **Diagnostic**: Index-predicted hits vs actual hits diverge by >10%.

### 6.3 Transfer Storm on Node Join
**Trigger**: New node joins cluster; consistent hashing reassigns 1/N of prefixes. All affected blocks begin transferring simultaneously, saturating network fabric. Active decode traffic suffers. **Symptom**: TPOT spikes cluster-wide during rebalancing window. **Diagnostic**: Network utilization jumps to >90% coincident with membership change.

### 6.4 Cascade Failure from Replication Loss
**Trigger**: Node holding sole replica of popular prefix fails. All dependent requests route to recompute, creating a thundering-herd prefill storm that overloads remaining compute. **Symptom**: Cluster-wide TTFT spike with prefill queue depth explosion. **Diagnostic**: Sudden jump in prefill-from-scratch rate coincident with node failure.

### 6.5 Coherence Violation (Silent Corruption)
**Trigger**: Two nodes independently compute the same prefix with different quantization settings or model versions (during rolling upgrade). Routing considers them equivalent. Decode uses wrong KV → attention computes incorrect outputs. **Symptom**: Output quality degradation without any system-level error signal. **Diagnostic**: Block version/config hash mismatch between replicas.

### 6.6 Over-Replication Memory Exhaustion
**Trigger**: Replication policy blindly replicates all popular prefixes without considering total cluster memory budget. Aggregate replicated blocks exceed available capacity, triggering cascading evictions of less popular but still valuable blocks. **Symptom**: Overall hit rate drops despite high replication factor; per-node eviction rate spikes. **Diagnostic**: Replicated block memory / total cluster KV memory > 50%.

### 6.7 Bandwidth-Bound Transfer Failure
**Trigger**: Disaggregated prefill-decode architecture attempts to transfer large KV blocks (70B model, long context) over insufficient interconnect. Transfer latency exceeds TTFT budget. System effectively degrades to colocated serving without the benefit. **Symptom**: TTFT ≈ recompute time despite cache hits; transfer queue depth grows unbounded. **Diagnostic**: `transfer_latency_p99 > TTFT_SLO * 0.5`.


## 7. Hypotheses the Agent Can Generate

1. **H1**: "Switching from round-robin to prefix-hash routing will increase cluster-wide KV hit rate by >40% because the current workload has >60% prefix sharing." (Testable: Compare hit rates under both routing strategies with identical workload.)

2. **H2**: "Replicating the top-5 system prompts to all nodes will reduce hotspot node utilization by >30% without reducing aggregate hit rate." (Testable: Measure per-node utilization variance before/after replication.)

3. **H3**: "The current centralized index lag (measured at ~50ms) causes >15% of routed requests to miss at the target node. Reducing index update latency to <10ms will recover those hits." (Testable: Instrument actual-vs-predicted hit rate at target nodes; correlate misses with index staleness.)

4. **H4**: "For prefix blocks under 512 tokens, local recompute is faster than cross-node transfer at our current 25 Gbps inter-node bandwidth. Blocks below this threshold should never be transferred." (Testable: Measure recompute time vs transfer time at various prefix lengths; find crossover.)

5. **H5**: "Disaggregating prefill to dedicated nodes and transferring KV to decode nodes will improve cluster throughput by >2× because our workload is 70% prefill-bound." (Testable: Compare throughput and SLO attainment in colocated vs disaggregated configurations.)

6. **H6**: "Adding a shared CPU-DRAM KV pool (LMCache-style) as cluster L2 will reduce recompute events by >50% because evicted blocks still have 30%+ probability of reuse within 60 seconds." (Testable: Log evicted blocks and track re-request time; measure recompute reduction with pool enabled.)

7. **H7**: "Compressing KV blocks 4× before cross-node transfer will make remote placement viable for blocks that currently exceed the transfer-latency budget." (Testable: Measure transfer time for compressed vs raw blocks; compare against TTFT budget.)


## 8. Experiments and Falsifiers

### E1: Routing Policy Comparison
- **Setup**: Same workload replayed under round-robin, prefix-hash, and index-based routing. Cluster of 8 nodes, production traffic mix.
- **Metric**: Hit rate, TTFT p50/p99, inter-node transfer volume.
- **Falsifier for H1**: If prefix-hash routing does not improve hit rate by >40%, or if load imbalance negates the hit rate gain (TTFT p99 worsens), H1 is falsified.

### E2: Replication Factor Sweep
- **Setup**: Vary replication factor R from 1 to N for the top-K prefixes. Measure per-node utilization spread and aggregate hit rate.
- **Metric**: Max-node utilization, utilization standard deviation, cluster hit rate, eviction rate.
- **Falsifier for H2**: If replication reduces hotspot utilization by <30%, or aggregate hit rate drops (replicas displace more valuable blocks), H2 is falsified.

### E3: Index Staleness Impact
- **Setup**: Instrument routing decisions with "index age" at decision time. Bucket outcomes by staleness (0-10ms, 10-50ms, 50-200ms). Compare predicted vs actual hit.
- **Metric**: Miss rate attributable to stale index entries.
- **Falsifier for H3**: If miss rate does not correlate with index age (misses are random, not staleness-caused), H3 is falsified.

### E4: Transfer-vs-Recompute Crossover
- **Setup**: For prefix lengths 128, 256, 512, 1024, 2048, 4096 tokens, measure both transfer time (at measured bandwidth) and recompute time (on idle GPU).
- **Metric**: Crossover prefix length where transfer < recompute.
- **Falsifier for H4**: If crossover is not near 512 tokens (much higher or lower), the threshold in H4 needs adjustment. If crossover is >4096 tokens, transfer is rarely worthwhile.

### E5: Disaggregation Throughput Test
- **Setup**: Deploy same model in colocated mode (prefill+decode on same GPUs) vs disaggregated (dedicated prefill pool, dedicated decode pool, RDMA transfer). Production workload mix.
- **Metric**: Total throughput (tokens/sec), TTFT, TPOT, GPU utilization per pool.
- **Falsifier for H5**: If disaggregated throughput improvement < 2×, or if KV transfer adds latency that violates TTFT SLO for >10% of requests, H5 is falsified.

### E6: Shared Pool Reuse Measurement
- **Setup**: Enable LMCache-style CPU-DRAM pool. Track blocks evicted from GPU tier that are subsequently re-requested. Compare recompute rate with pool vs without.
- **Metric**: Pool hit rate, recompute reduction, TTFT improvement.
- **Falsifier for H6**: If <30% of evicted blocks are re-requested within the pool's TTL, or pool access latency causes TTFT regression, H6 is falsified.

### E7: Compressed Transfer Viability
- **Setup**: Compress KV blocks using CacheGen-style encoding (4× target). Measure encode time + transfer time + decode time vs raw transfer time vs recompute time.
- **Metric**: End-to-end latency for compressed-transfer path; accuracy impact.
- **Falsifier for H7**: If encode + compressed-transfer + decode > raw recompute, compression does not help transfer viability. If accuracy drops measurably, compression is not viable for this use.


## 9. Production Evidence

### 9.1 Mooncake: KVCache-Centric Disaggregated Scheduling
- **System**: Mooncake (Kimi/Moonshot AI production serving platform)
- **Problem**: Colocated prefill+decode on shared GPUs creates resource contention; GPU DRAM alone cannot hold the working set for long-context traffic.
- **Mechanism**: Separated prefill and decode clusters; built a distributed KVCache pool leveraging underutilized CPU DRAM and SSD across the GPU cluster; KVCache-centric scheduler routes requests to maximize cache reuse while meeting SLOs.
- **Result**: 525% throughput increase in simulated scenarios; 75% more requests served in production under SLO constraints. Excels in long-context scenarios.
- **Lesson**: Distributed KV ownership with a dedicated scheduling layer that jointly optimizes cache hit and load balance dramatically outperforms colocated serving. The "KVCache-centric" framing—treating KV placement as the primary scheduling axis—proves correct at scale.

### 9.2 llm-d: Global KV Index with Prefix-Aware Routing
- **System**: llm-d (CNCF sandbox; Red Hat, Google Cloud, IBM Research, CoreWeave, NVIDIA)
- **Problem**: Round-robin routing to vLLM instances wastes prefix cache hits; single-node GPU memory limits effective working set for multi-turn traffic.
- **Mechanism**: Global KV cache index tracks block locations across cluster; prefix-cache-aware router directs requests to nodes holding matching prefixes; hierarchical offloading (GPU→CPU→disk) expands per-node capacity.
- **Result**: 3× higher output throughput and 2× faster TTFT vs round-robin routing. 13.9× throughput improvement with hierarchical KV offloading at 250 concurrent users vs GPU-only. Prefill-decode disaggregation achieved up to 70% higher tokens/sec on NVIDIA B200.
- **Lesson**: A global index that maps prefix hashes to physical locations enables routing decisions that would be impossible with local-only cache state. The combination of intelligent routing + tiered offloading multiplies effective cluster capacity.

### 9.3 DistServe: Disaggregated Prefill-Decode Pools
- **System**: DistServe (OSDI 2024)
- **Problem**: Colocating prefill and decode on the same GPUs creates interference—optimizing TTFT conflicts with optimizing TPOT; resource allocation is coupled.
- **Mechanism**: Separate GPU pools for prefill (compute-optimized parallelism) and decode (memory-bandwidth-optimized parallelism); KV transfer between pools; independent resource allocation and scaling per phase.
- **Result**: Serves 7.4× more requests or achieves 12.6× tighter SLO bounds compared to colocated state-of-the-art, while meeting latency constraints for >90% of requests.
- **Lesson**: Phase-disaggregation creates a KV ownership transfer problem (prefill must hand KV to decode), but the performance gains from independent optimization far outweigh transfer costs when interconnect bandwidth is sufficient.

### 9.4 Splitwise: Phase Splitting with KV Transfer
- **System**: Splitwise (ISCA 2024)
- **Problem**: Decode phase does not need latest-generation GPU compute capability; homogeneous clusters waste expensive hardware on memory-bandwidth-bound decode.
- **Mechanism**: Split LLM inference across machines tailored to each phase; optimize KV state transfer using fast back-plane interconnects (NVLink, InfiniBand).
- **Result**: 1.4× higher throughput at 20% lower cost; or 2.35× more throughput at same cost and power budget. Heterogeneous machine types per phase unlock cost-performance Pareto improvements.
- **Lesson**: KV transfer cost is acceptable when interconnect is fast relative to the prefix length. The ownership model (prefill-owns → transfer → decode-owns) is simple and sufficient when coherence only needs to be maintained per-sequence.

### 9.5 SGLang RadixAttention: Implicit Locality via Tree Structure
- **System**: SGLang (2024)
- **Problem**: Requests in complex LM programs (agents, few-shot, multi-turn) share large prefix segments, but standard KV caches discard them between requests.
- **Mechanism**: Radix tree indexes KV blocks by token prefix. Automatic prefix sharing: any request matching an existing prefix reuses cached KV. Implicit locality—blocks are co-owned by the tree structure, not explicitly placed.
- **Result**: Up to 6.4× higher throughput vs prior inference systems across agent, reasoning, few-shot, and RAG workloads.
- **Lesson**: For single-node or affinity-routed systems, structural ownership (radix tree) eliminates explicit placement decisions entirely. The tree encodes sharing relationships as its topology. This pattern extends to distributed systems as a per-node local policy, with cross-node routing layered above.

### 9.6 vLLM PagedAttention: Block-Granular Memory Management
- **System**: vLLM (SOSP 2023)
- **Problem**: KV cache memory is wasted through fragmentation and redundant duplication, limiting batch size and throughput.
- **Mechanism**: OS-inspired paged memory for KV blocks; virtual-to-physical mapping enables zero-copy sharing of common prefix pages across requests; near-zero fragmentation.
- **Result**: 2–4× throughput improvement over FasterTransformer and Orca with same latency; larger gains with longer sequences and larger models.
- **Lesson**: Block-granular ownership with reference counting enables safe sharing without explicit replication. The "page table" abstraction translates directly to distributed settings: a global page table mapping virtual KV block IDs to physical (node, address) pairs.

### 9.7 CacheGen: Compressed KV Transfer
- **System**: CacheGen (SIGCOMM 2024)
- **Problem**: Reusing cached KV from remote storage requires transferring large tensors over the network; fetch latency dominates TTFT for long contexts.
- **Mechanism**: Custom tensor encoder exploiting KV distribution properties for 3.5–4.3× compression; adaptive compression level based on available bandwidth.
- **Result**: 3.5–4.3× KV cache size reduction; 3.2–3.7× total delay reduction for context loading with negligible quality impact.
- **Lesson**: Compression transforms the placement decision space. Blocks that were "too expensive to transfer" become viable remote candidates. This effectively expands the radius of distributed ownership—remote nodes can serve compressed KV within the latency budget that raw transfer would violate.

### 9.8 InfiniGen: Selective Retrieval from Offloaded KV
- **System**: InfiniGen (OSDI 2024)
- **Problem**: Offloading full KV to host memory helps capacity but fetch latency for retrieving all layers is prohibitive.
- **Mechanism**: Speculative rehearsal: uses current-layer inputs + partial next-layer weights to predict which KV entries are essential. Prefetches only critical entries from host memory.
- **Result**: 3× performance improvement over prior offloading-based KV management with substantially better model accuracy.
- **Lesson**: Ownership need not be all-or-nothing. Selective retrieval means a remote/offloaded owner can serve partial KV (critical heads/layers only), reducing effective transfer size by 60–80%. Distributed ownership benefits from selective fetch: route requests to nodes that hold the critical subset, not necessarily the complete block.

### 9.9 LMCache: Engine-Independent Distributed KV Daemon
- **System**: LMCache (PyTorch Foundation ecosystem; production at CoreWeave/Cohere)
- **Problem**: KV cache tied to inference engine lifetime; engine crash loses all cached state; single-engine memory bounds limit prefix reuse.
- **Mechanism**: Standalone daemon process manages KV independently of engine. Multi-node P2P CPU memory sharing; pluggable backends (Redis, Mooncake store, S3, NIXL). Engine-independent means KV survives crashes and can be accessed by any engine instance.
- **Result**: Enables cross-engine KV sharing; KV survives engine crashes; multi-node sharing moves from experimental to production (2026). 10× MoE inference performance with multiprocess architecture.
- **Lesson**: Decoupling KV ownership from the inference engine process is the fundamental architectural decision. Once KV is externally owned, distributed placement, replication, and failover become storage problems solvable with proven distributed systems techniques rather than engine-specific hacks.


## 10. Implications for KV Block Storage

1. **Content-addressable blocks are the natural sharing primitive**: Prefix blocks are immutable and identified by token-sequence hash. A storage system should treat them as content-addressed objects—placement and replication follow from hash-based routing, deduplication is free, and coherence is trivial (immutable objects cannot become stale).

2. **The storage layer must expose a global index API**: Routing decisions require knowing what's cached where. The storage system must maintain and expose a low-latency lookup: `prefix_hash → [(node, tier, offset)]`. This is the distributed equivalent of a page table.

3. **Transfer bandwidth is the binding constraint, not capacity**: With CPU DRAM and SSD tiers available (100s of GB to TB per node), raw capacity is plentiful. The scarce resource is the bandwidth to move KV between tiers and nodes within TTFT budgets. Storage design must optimize for transfer throughput: large sequential reads, compressed encoding, selective retrieval.

4. **Replication should be demand-driven, not preconfigured**: Static replication wastes memory on cold prefixes and under-replicates hot ones. The storage layer should support dynamic replication: promote to replica status when access rate exceeds single-node capacity, demote when rate drops.

5. **Failover is a storage responsibility**: When KV ownership is distributed, node failure creates a data-availability problem. The storage layer must provide either replicated reads (for prefix blocks) or fast recompute triggers (for suffix blocks) with bounded recovery time within the TTFT SLO.

6. **Compression changes the placement Pareto frontier**: 3.5–4.3× compression (CacheGen) means blocks that exceed the raw-transfer budget become viable remote candidates. The storage system should support transparent compress-on-write for remote/offloaded tiers, expanding the effective radius of placement decisions.

7. **Coherence model is bimodal**: Prefix blocks are immutable (content-addressed, share freely). Suffix/decode blocks are mutable and session-bound (single-writer, no sharing). The storage system needs two coherence policies, not one: broadcast invalidation is unnecessary for prefixes; strict single-ownership suffices for suffixes.

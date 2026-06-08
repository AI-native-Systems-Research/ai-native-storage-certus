# Inference-Native Storage Diagnostic Knowledge Base

Given runtime observations, this KB encodes reasoning rules for diagnosing bottlenecks,
estimating theoretical limits, and recommending optimizations. The profiling agent uses
this to reason from **symptoms** rather than just reporting metrics.

> All numeric thresholds (80%, 50%, 64KB, 1MB, 85%, 100μs, QD=64+, etc.) are initial
> heuristics and should be calibrated per platform, model, GPU, SSD, and workload.

---

## 1. Hardware Models

### NVMe Queue Depth

**Symptom:** NVMe bandwidth below device peak, stable latency, low CPU utilization.

**Possible causes:**
1. Queue depth too low (drive not saturated)
2. I/O size too small (overhead per-IO dominates)
3. Software serialization between IOs (lock, channel wait)

**Models to check:**
- `bandwidth ≈ IOPS × avg_IO_size`
- `IOPS ≈ avg_inflight_IOs / avg_latency` (Little's Law — use actual in-flight count, not configured iodepth)
- NVMe saturation curve: does throughput plateau at current QD?

**Experiments:**
- Sweep iodepth and numjobs with fio (one job may not saturate even at high QD):
  `iodepth={1,2,4,8,16,32,64,128} × numjobs={1,2,4,8}`
- Compare achieved QD vs submitted QD (are completions backing up?)
- Check if single-threaded submission is the limit

**Rules:**
```
IF bandwidth < 80% of device_peak
AND avg_latency is stable (not growing)
AND CPU_util < 50%
THEN increase I/O concurrency (more qpairs, higher QD, or more threads)
     before changing layout or IO size.
```

### SSD Bandwidth/Latency Curves

**Symptom:** Throughput scales with drives but sub-linearly.

**Possible causes:**
1. Shared PCIe root complex saturated
2. CPU-side serialization (single dispatch thread)
3. Memory bandwidth (DMA buffer copies through DRAM)
4. NUMA cross-socket access (drives on remote node)

**Experiments:**
- Add drives one at a time, measure incremental bandwidth
- Pin drives to local NUMA node, compare
- Check PCIe topology/link width/link speed with `lspci`; monitor utilization with `perf`/uncore counters, NVIDIA tools, or vendor-specific telemetry

**Rules:**
```
IF N_drives × per_drive_BW > PCIe_root_complex_BW
THEN PCIe is the ceiling — P2P or multi-root won't help without topology change.

IF adding drives from remote NUMA shows <50% incremental gain
THEN NUMA placement is the issue — pin buffers to local node.
```

### PCIe/GPU DMA Efficiency

**Symptom:** GPU H2D bandwidth below PCIe link theoretical max.

**Possible causes:**
1. Small transfer sizes (TLP overhead)
2. Pageable memory (bounce buffer staging)
3. Synchronous copies (waiting for each before next)
4. GPU busy with compute (DMA engine sharing)

**Experiments:**
- `cuda_bandwidth_test` with varying sizes
- Compare pinned vs pageable
- Compare sync vs async memcpy
- Check nvidia-smi for DMA engine utilization

**Rules:**
```
IF transfer_size < 64KB AND bandwidth < 50% of link_peak
THEN coalesce transfers — batch multiple small KV into one DMA.

IF pageable_memory is used on hot path
THEN use pinned/page-locked memory — avoids driver-managed staging through bounce buffers
     and enables higher-bandwidth async DMA.

IF synchronous memcpy on hot path
THEN switch to async + stream — overlap transfer with compute.
```

### GPUDirect / P2P Constraints

**Symptom:** GPUDirect/P2P capability appears available, but the hot path still stages data through host DRAM.

**Possible causes:**
1. Code never calls P2P/GDS functions (unused capability)
2. BAR1 aperture too small for working set
3. PCIe topology prevents peer access (ACS enabled, wrong root port)
4. IOMMU configuration blocking DMA

**Experiments:**
- For GDS (storage→GPU): verify `nvidia-fs` module, use `gdscheck` / `gdsio` to validate path
- For RDMA/P2P (GPU↔GPU or NIC→GPU): verify `nvidia-peermem` and peer access support
- Check BAR1 size: `nvidia-smi -q | grep BAR1`
- Test GPU-GPU P2P: `cuda_sample p2pBandwidthLatencyTest`
- Check ACS: `setpci -s <bridge> ECAP_ACS+6.w`

**Rules:**
```
IF GPUDirect/P2P capability is available
AND the code path does not use the corresponding GDS/RDMA/P2P API
THEN the system will still stage through host DRAM — implement the direct path
     or document why it is unsupported on this topology.

IF BAR1_size < working_set_size
THEN direct peer mappings may only cover a subset — prioritize hot objects for direct access
     and fall back to staged transfers for cold objects.
```

---

## 2. Software-Path Models

### Syscall / Copy Overhead

**Symptom:** High CPU utilization but low device utilization.

**Possible causes:**
1. Too many memcpy operations per request
2. Per-IO metadata serialization/deserialization
3. Frequent small allocations (malloc/free per IO)
4. Lock acquisition in hot path

**Models to check:**
- Count copies on data path: each copy = CPU time + memory bandwidth
- `software_overhead_per_IO = CPU_time_per_IO - device_service_time`

**Experiments:**
- `perf record` on hot path → identify top CPU consumers
- Count memcpy calls per lookup (read code or instrument)
- Measure with/without metadata operations

**Rules:**
```
IF CPU_time_per_IO > 2× device_latency
THEN software overhead dominates — optimize CPU path before hardware path.

IF memcpy_count_per_request > 1
THEN look for zero-copy alternatives (DMA directly to destination, aliased buffers).
```

### Lock / Thread Contention

**Symptom:** Throughput stops scaling with client count. CPU utilization high but not 100%.

**Possible causes:**
1. Single mutex protecting shared state (convoy effect)
2. False sharing on adjacent cache lines
3. Condvar with thundering herd on wakeup
4. Reader-writer lock with write-biased starvation

**Experiments:**
- `perf lock` or `lockstat` to measure contention
- Vary client count: 1, 2, 4, 8, 16 — find the cliff
- Instrument: time spent waiting vs time spent working

**Rules:**
```
IF throughput_at_N_clients < N × throughput_at_1_client × 0.7
AND CPU utilization < 90%
THEN lock contention — identify the shared resource.

IF critical_section is very short
AND threads are not oversubscribed
AND contention duration is bounded
THEN evaluate spinlock, adaptive mutex, or sharded locking
     (spinlocks degrade under oversubscription, preemption, NUMA effects, or tail-heavy sections).

IF readers >> writers
THEN RwLock or lock-free reads (atomics, epoch-based).
```

### Metadata Lookup Cost

**Symptom:** High per-operation latency even for hot (memory-tier) requests.

**Possible causes:**
1. Hash table with poor distribution → chain traversal
2. Lock on lookup path (readers block on writers)
3. Cache-cold metadata (NUMA remote, evicted from CPU cache)

**Rules:**
```
IF hot_path_latency > expected_DMA_time + 10μs
THEN metadata lookup overhead is significant — profile the dispatch-map path.

IF dispatch_map uses condvar wait
THEN readers block on writers — consider optimistic read or per-entry lock.
```

---

## 3. Object / Layout Models

### Object Size Effects

**Symptom:** High IOPS but low bandwidth, or high bandwidth but high read amplification.

**Models to check:**
- `bandwidth = IOPS × object_size`
- `read_amp = bytes_read / bytes_consumed`
- Per-IO fixed cost amortization: `effective_bw = size / (fixed_cost + size / raw_bandwidth)`

**Rules:**
```
IF object_size < 64KB AND bandwidth is the goal
THEN coalesce adjacent objects into larger read units.

IF object_size > 1MB AND GPU only uses a fraction
THEN read_amp is high — split objects or use sub-object access.
```

### Fragmentation / Layout

**Symptom:** Sequential bandwidth drops over time, or after many promotes/evicts.

**Possible causes:**
1. Physical fragmentation on SSD (non-contiguous extents)
2. Logical vs physical mismatch (layer-major stored token-major)
3. Free-list fragmentation in memory tier

**Rules:**
```
IF fresh_system_bandwidth > aged_system_bandwidth × 1.3
THEN fragmentation is accumulating — consider compaction or append-only layout.

IF prefill_throughput < decode_throughput (unexpected)
THEN layout may be optimized for decode but not prefill — check layer-major vs token-major.
```

### Read Amplification

**Symptom:** SSD reads much more data than GPU consumes.

**Possible causes:**
1. Block-aligned reads fetch padding bytes
2. Full-object fetch when only partial needed (e.g. single attention head)
3. Metadata co-stored with data (read together)

**Rules:**
```
IF read_amp > 1.5
THEN significant waste — consider sub-object access, separate metadata, or aligned packing.
```

---

## 4. Cache / Tier Models

### Hit Rate

**Symptom:** High cold-path traffic despite repeated access to same keys.

**Possible causes:**
1. Cache too small for working set
2. Poor admission policy (caching one-shot keys)
3. Premature eviction (thrashing under multi-tenant load)

**Models to check:**
- `hit_rate ≈ min(1, cache_size / working_set_size)` (uniform, toy)
- Actual hit rate from metrics vs model prediction → gap = policy problem

**Rules:**
```
IF measured_hit_rate < predicted_hit_rate(cache_size, working_set)
THEN admission or eviction policy is suboptimal — not a sizing problem.

IF hit_rate is high but cold_path_latency still dominates overall
THEN by Amdahl's Law, optimize the cold path (even rare misses dominate if slow enough).
```

### Eviction Pressure

**Symptom:** High p99 latency spikes, correlated with promote operations.

**Possible causes:**
1. Eviction in-line with promote (synchronous)
2. Lock contention during eviction scan
3. Eviction cascade (evicting entry that another thread is about to read)

**Rules:**
```
IF p99_spike correlates with eviction activity
THEN move eviction to background (pre-evict to maintain headroom).

IF eviction_scan_time > 100μs
THEN scan window too large or lock held too long — use smaller batches.
```

### Promotion Cost

**Symptom:** Cold-path latency dominated by overhead, not actual data transfer.

**Possible causes:**
1. Metadata update in critical path (dispatch-map write lock)
2. Memory-tier allocation under lock
3. Multiple round-trips (allocate → read → copy → register)

**Rules:**
```
IF promote_latency > SSD_read_time + DMA_time + 50%
THEN overhead dominates — pipeline the metadata/allocation ahead of data transfer.
```

---

## 5. Inference-Specific Models

### Prefill vs Decode

**Symptom:** System optimized for one pattern but not the other.

**Characteristics:**
- Prefill: bulk sequential, all layers, large batch of tokens → bandwidth-bound
- Decode: small, repeated, same keys, single token at a time → latency-bound

**Rules:**
```
IF workload is prefill-heavy (long contexts, new sessions)
THEN optimize for sequential bandwidth: larger IOs, prefetch, layout aligned with
     the engine's actual layer/token/page access order.

IF workload is decode-heavy (ongoing generation, chat)
THEN optimize for latency: keep hot KV in fast tier, minimize per-access overhead.
```

### KV Reuse Probability

**Symptom:** Cache stores KV that is never reused, wasting tier capacity.

**Models to check:**
- `P(reuse) = f(session_type, context_length, time_since_last_access)`
- Shared prefixes across users have high reuse
- One-shot queries have zero reuse

**Rules:**
```
IF P(reuse) < threshold AND recompute_cost < reload_cost
THEN do not persist — recompute when needed.

IF shared_prefix_detected (multiple sessions share prompt prefix)
THEN high reuse value — prioritize in cache, never evict while sessions active.
```

### Recompute vs Reload Decision

**Symptom:** System always stores/reloads when recomputing would be cheaper.

**Models:**
- `reload_cost = SSD_read + DMA + metadata + sync`
- `recompute_cost = prefill_FLOPs / GPU_throughput + GPU_opportunity_cost`
- `net_value = P(reuse) × (recompute_cost - reload_cost) - storage_cost`

**Rules:**
```
IF estimated_recompute_cost < estimated_reload_cost
THEN recompute rather than persist/reload.
(Example threshold: context < ~512 tokens with a small model often favors recompute.)

IF long context AND reuse is likely AND reload_cost < recompute_cost
THEN persist and prioritize in cache.
(Example threshold: context > ~4K tokens with likely reuse often favors reload.)

IF GPU_utilization > 90%
THEN recompute has high opportunity cost — prefer reload even for shorter contexts.
```

### SLO Pressure

**Symptom:** Individual requests meet SLO but tail degrades under load.

**Rules:**
```
IF p99 > SLO AND median < SLO × 0.5
THEN tail is the problem — focus on interference, eviction storms, lock convoys.

IF median approaches SLO
THEN fundamental path is too slow — need architectural change (P2P, better layout).
```

---

## 6. Correctness / Semantic Identity

### KV Validity

**Symptom:** Inference quality degrades after KV cache reload (subtle, no crash).

**Possible causes:**
1. KV generated with different model checkpoint
2. Tokenizer version mismatch
3. Position encoding parameters changed
4. dtype/quantization level changed
5. Attention layout (MHA vs GQA vs MQA) mismatch

**Rules:**
```
IF cache_key does NOT include {model_version, tokenizer_hash, prompt_prefix_hash,
   position_range, pos_enc_params, rope_scaling_config, dtype, quantization_format,
   attn_layout, layer_range, head_range, kv_block_size}
THEN semantic identity is not enforced — stale or mismatched KV may be served silently.

IF model updated AND cache not invalidated
THEN ALL cached KV for that model is potentially stale — must flush or version-tag.
```

---

## 7. Action Rules

Consolidated decision rules for common scenarios.

### When to Prefetch
```
IF access pattern is predictable (layer-sequential, known future tokens)
AND prefetch_cost < stall_cost (GPU would otherwise idle)
AND tier has bandwidth headroom
THEN prefetch next N layers/tokens into memory tier ahead of demand.
```

### When to Promote
```
IF key is accessed AND not in fast tier
AND P(future_reuse) × expected_future_savings > promotion_cost
THEN promote to memory tier.

IF key is accessed AND in memory tier but not GPU
AND active inference needs it within next K tokens
THEN DMA to GPU asynchronously (pipeline with compute).
```

### When to Evict
```
IF memory_tier_utilization > 85%
THEN proactively evict (don't wait for allocation failure).

IF evicting, prefer:
  1. Keys with lowest P(reuse) — LRU approximation
  2. Keys where recompute_cost < reload_cost (cheap to regenerate)
  3. Keys from inactive sessions (no recent decode activity)
  
DO NOT evict keys actively being attended to (check ref-count).
```

### When to Recompute (Instead of Reload)
```
IF reload_cost > recompute_cost
OR key is stale (model/tokenizer updated)
OR GPU has idle cycles (low utilization)
THEN recompute from prompt — don't read from storage.
```

### When to Compress
```
IF memory_tier_pressure is high
AND expected saved reload/recompute cost exceeds compression + decompression overhead
AND accuracy loss is within tolerance
THEN compress or quantize KV in tier.
```

### When to Relayout
```
IF access pattern has changed (was prefill-heavy, now decode-heavy)
AND current layout causes read_amp > 2.0
AND background I/O bandwidth is available
THEN compact/relayout in background to match new access pattern.
```

### When to Increase Concurrency
```
IF throughput < ceiling
AND latency is stable (not growing)
AND CPU utilization < 80%
THEN bottleneck is concurrency — increase QD, threads, streams, or qpairs
     BEFORE changing architecture or layout.
```

### When to Change Architecture
```
IF throughput < 50% of hardware ceiling
AND concurrency is already saturated (QD=64+, multiple threads)
AND CPU utilization is high on copy/overhead operations
THEN the data path itself is wrong — need architectural change
     (P2P bypass, zero-copy, different tier layout, eliminate bounce).
```

---

## 8. Distributed Serving / Multi-Resource Models

### 8.1 RPC / gRPC Path

**Symptom:** Server-side storage/GPU metrics look healthy, but end-to-end latency is high.

**Possible causes:**
1. gRPC/protobuf serialization overhead
2. Client-side request generation bottleneck
3. Connection pool too small
4. Head-of-line blocking on shared channel
5. Too many small RPCs instead of batched requests
6. Server receive/completion queue saturation
7. Backpressure from slow clients

**Models to check:**
- `end_to_end_latency = client_queue + serialization + network + server_queue + service_time + response_serialization`
- `rpc_throughput ≈ active_streams / avg_rpc_latency`
- `useful_payload_ratio = useful_bytes / serialized_rpc_bytes`

**Experiments:**
- Measure client-side latency vs server-side latency (instrument both)
- Compare unary RPC vs batched/streaming RPC
- Sweep number of client connections/channels
- Measure protobuf encode/decode time
- Measure request payload size distribution

**Rules:**
```
IF server_latency is low
AND end_to_end_latency is high
THEN bottleneck is outside the storage path — inspect client, RPC, network, serialization.

IF payload_size is small AND RPC_count is high
THEN batch requests or use streaming RPC.

IF throughput stops scaling with clients
AND server CPU/GPU/storage are not saturated
THEN client-side generation, gRPC channel contention, or network stack is the bottleneck.
```

### 8.2 Multi-Client Scaling

**Symptom:** Single client performs well, but throughput or p99 collapses with many clients.

**Possible causes:**
1. Shared metadata lock (dispatch-map, memory-tier)
2. Shared gRPC completion queue
3. Server thread-pool saturation
4. Cache thrashing across clients
5. Eviction/promote storms
6. Unfair scheduling between clients
7. Client-side coordinated omission

**Models to check:**
- `per_client_QPS` vs `aggregate_QPS`
- `fairness = min_client_throughput / max_client_throughput`
- `cache_interference = hit_rate_single_client - hit_rate_multi_client`

**Rules:**
```
IF aggregate_QPS increases but per-client p99 explodes
THEN system is throughput-scaling but not latency-isolated.

IF one client's workload reduces another client's hit rate
THEN cache interference — need admission control, partitioning, or priority-aware eviction.

IF p99 spikes correlate across all clients simultaneously
THEN shared bottleneck: storage, PCIe, metadata lock, RPC queue, or eviction storm.

IF only one client has high p99
THEN check routing, client-side queueing, or per-session placement.
```

### 8.3 Multi-Drive Scheduling

**Symptom:** Multiple SSDs do not scale linearly.

**Possible causes:**
1. Shared PCIe root complex saturation
2. Uneven object placement across drives
3. Single submission thread bottleneck
4. Per-drive queue depth too low
5. NUMA mismatch between drive, CPU thread, and DMA buffer
6. Hot objects concentrated on one drive

**Rules:**
```
IF drive_utilization is imbalanced
THEN object placement or scheduler is causing hot-spotting.

IF all drives show low utilization AND request queue is non-empty
THEN submission path or metadata path is the bottleneck.

IF one drive is saturated AND others are idle
THEN rebalance objects or use request striping.

IF per_drive_QD is low
THEN increasing total system QD may not help unless QD is distributed across drives.
```

### 8.4 Multi-GPU / GPU-Locality

**Symptom:** Aggregate GPU utilization is low or uneven, despite enough requests.

**Possible causes:**
1. KV placed on wrong GPU (locality miss)
2. Cross-GPU KV migration over PCIe (no NVLink)
3. Load imbalance across GPUs
4. One GPU owns all hot cache entries
5. P2P disabled or topology-limited between GPUs
6. GPU memory fragmentation
7. Scheduler ignores KV locality
8. Tensor-parallel or pipeline-parallel placement constrains where KV can be consumed

**Models to check:**
- `local_hit_rate` vs `remote_hit_rate` per GPU
- `cross_gpu_transfer_bytes`
- `placement_cost = local_access_cost vs remote_access_cost vs recompute_cost`

**Rules:**
```
IF request needs KV on GPU A AND compute runs on GPU B
THEN either migrate KV, route request to GPU A, or recompute locally.

IF cross_gpu_transfer_bytes is high AND NVLink is unavailable
THEN KV locality-aware scheduling is critical.

IF one GPU has high memory pressure AND others are underutilized
THEN rebalance KV placement or use per-GPU cache quotas.

IF remote_KV_access_cost > recompute_cost
THEN recompute locally instead of migrating KV.
```

### 8.5 Cross-Resource Bottleneck Attribution

**Symptom:** Performance is below expectation but no single resource looks saturated.

**Rules:**
```
IF storage is idle AND GPU is idle AND client latency is high
THEN bottleneck is RPC/client/server scheduling, not hardware.

IF storage is saturated AND GPU is idle
THEN cold-path storage is starving compute — optimize storage or increase caching.

IF GPU is saturated AND storage is idle
THEN compute dominates — caching may not help; need faster model or more GPUs.

IF PCIe is saturated AND storage/GPU have headroom
THEN data movement is the bottleneck — need P2P, compression, or reduced transfer volume.

IF CPU is saturated AND storage/GPU/PCIe have headroom
THEN software overhead dominates — optimize metadata, locks, copies, or serialization.
```

---

## 9. Measurement Validity / Benchmark Pitfalls

### Coordinated Omission
```
IF benchmark issues requests sequentially (closed-loop)
THEN p99 may be hidden — slow responses delay subsequent requests, hiding queue buildup.
USE open-loop / constant-rate load generation for realistic latency measurement.
```

### Cache Warming Bias
```
IF cache is warmed before measurement
THEN cold-path behavior is not represented in results.
MEASURE both cold-start and steady-state separately.
```

### Synthetic Access Patterns
```
IF object IDs are uniform random
THEN reuse behavior may not match real agent/chat workloads (which have skew, locality, shared prefixes).
USE trace replay or realistic access generators.
```

### Target-Limited Throughput
```
IF benchmark throughput matches request injection rate exactly
THEN system is not saturated — measured latency reflects unsaturated behavior.
INCREASE load until throughput plateaus to find the saturation point.
```

### Measurement Interference
```
IF profiling/instrumentation is active during measurement
THEN overhead may distort results (especially for latency-sensitive paths).
MEASURE with and without instrumentation; report the delta.
```

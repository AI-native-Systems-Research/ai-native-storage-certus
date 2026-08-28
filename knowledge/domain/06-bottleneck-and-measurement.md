I have all the context needed. I can see the format, production evidence style, and specific numbers from the existing files. Let me now output the complete domain reference file.

---

```markdown
---
id: bottleneck-and-measurement
decision: Identify what resource or interaction is actually limiting system throughput or latency, and design experiments that causally prove the bottleneck before committing optimization effort
answers:
  - What is actually limiting performance, and how do we prove it?
  - Is the system compute-bound, memory-bandwidth-bound, IO-bound, or coordination-bound?
  - How much improvement is theoretically possible if the identified bottleneck is removed?
  - Are multiple bottlenecks interacting, and in what order should they be attacked?
  - How do we distinguish correlation from causation in performance observations?
inputs:
  - per-tier utilization (GPU compute, HBM bandwidth, PCIe, NVMe, network)
  - request-level latency breakdown (prefill, decode, queue, transfer)
  - operational intensity (FLOPs per byte moved) of the active workload
  - queue depths and occupancy at each pipeline stage
  - throughput as function of offered load (saturation curve)
  - SLO headroom per request (TTFT budget minus actual)
  - parallelism fraction of the critical path
  - concurrent request count and batch size
owns: ['roofline', 'Amdahl', "Little's Law", 'paired experiments', 'causal diagnosis']
excludes:
  - how to move data efficiently once the bottleneck is known (see data-movement-concurrency)
  - which blocks to evict or promote (see tier-policy-and-eviction)
  - placement and routing decisions (see distributed-kv-ownership)
  - hardware specifications and device internals
  - attention kernel implementation details
related:
  - data-movement-concurrency
  - workload-to-storage-io
  - tier-policy-and-eviction
  - cache-value-and-recompute
  - distributed-kv-ownership
---

## 1. Decision Being Made

The optimization agent must decide:

1. **Where to focus effort**: Which resource or interaction is the true limiting factor—GPU compute, memory bandwidth, storage IO, network transfer, or software coordination (locks, scheduling, GC)?
2. **How much improvement is available**: What is the theoretical ceiling if this bottleneck is eliminated entirely, and is that ceiling worth pursuing?
3. **Whether the diagnosis is causal**: Can we design an experiment that proves the bottleneck (removing it improves performance) versus merely correlating with poor performance?
4. **Attack ordering**: When multiple bottlenecks interact (Amdahl cascades), which must be relieved first to unlock improvement from the next?
5. **Measurement fidelity**: Are the observations trustworthy, or are measurement artifacts (probe overhead, cold caches, coordinated omission) creating false signals?

Getting this wrong wastes optimization cycles on non-bottlenecks (Amdahl guarantees zero improvement on non-critical paths), or worse, shifts the bottleneck to a harder-to-fix resource by optimizing the wrong one.

## 2. Mental Model and Equations

### Roofline Model for KV Cache Operations

```
attainable_perf = min(peak_compute, peak_bandwidth × operational_intensity)
```

Where:
- `peak_compute` = device FLOP/s (e.g., GPU FP16 throughput)
- `peak_bandwidth` = memory or IO bandwidth ceiling (HBM, PCIe, NVMe)
- `operational_intensity` = FLOPs per byte accessed (attention FLOPs / KV bytes loaded)

For KV cache attention (decode phase, single token attending to cached keys/values):
```
OI_decode = (2 × seq_len × d_head) FLOPs / (2 × seq_len × d_head × sizeof(dtype)) bytes
           = 1 / sizeof(dtype)
           ≈ 0.5 FLOP/byte (FP16) or 1.0 FLOP/byte (FP8/INT8)
```

This is extremely low operational intensity — decode attention is almost always **memory-bandwidth-bound**, not compute-bound. The roofline tells the agent: optimizing compute for decode is futile; only reducing bytes touched or increasing bandwidth helps.

For prefill phase (processing N input tokens):
```
OI_prefill = (2 × N² × d_head) FLOPs / (2 × N × d_head × sizeof(dtype)) bytes
            = N / sizeof(dtype)
```

Prefill crosses from bandwidth-bound to compute-bound when N exceeds the device's compute-to-bandwidth ratio (the "ridge point"). For A100 at FP16: ridge ≈ 156 tokens; for H100: ridge ≈ 256 tokens.

### Amdahl's Law for Sequential-Parallel Decomposition

```
speedup_max = 1 / (s + (1 - s) / N)
```

Where:
- `s` = fraction of time spent in the serial (unparallelizable) portion
- `N` = parallelism applied to the parallel portion
- As N → ∞: `speedup_max → 1/s`

**Applied to KV cache storage**: If prefill compute is 40% of end-to-end latency and KV transfer is 60%, even infinite transfer speedup yields at most 1/0.4 = 2.5× improvement. Conversely, if KV transfer is only 10% of latency, no storage optimization can yield more than 1/0.9 = 1.11× improvement—the agent should look elsewhere.

### Generalized Amdahl for Multiple Bottlenecks

```
T_total = T_1 + T_2 + ... + T_k  (serial composition)
speedup_from_fixing_i = T_total / (T_total - T_i × (1 - 1/improvement_i))
```

This tells the agent the priority order: fix the largest T_i first, because its removal exposes the next bottleneck.

### Little's Law for System Capacity

```
L = λ × W
```

Where:
- `L` = number of in-flight requests (concurrency)
- `λ` = throughput (requests/s)
- `W` = mean service time (latency in seconds)

Rearranged for capacity planning:
```
λ_max = L_max / W_min
```

If the system can hold at most 32 concurrent requests (GPU memory limit) and each request takes 50 ms decode step: `λ_max = 32 / 0.05 = 640 steps/s`. To increase throughput, the agent must either increase L_max (more memory → larger batch) or decrease W (faster per-step latency).

### Utilization Law

```
U_resource = λ × S_resource
```

Where `S_resource` is the mean service time at that resource per request. A resource is the bottleneck when `U_resource → 1.0`. Multiple resources at high utilization signal cascading contention.

### Variance and Tail Latency (Kingman's Formula)

```
W_queue ≈ (ρ / (1 - ρ)) × (C_a² + C_s²) / 2 × S
```

Where:
- `ρ` = utilization
- `C_a` = coefficient of variation of arrival process
- `C_s` = coefficient of variation of service time
- `S` = mean service time

This explains why systems at 80% utilization have tolerable p50 but terrible p99: variance in arrivals or service amplifies queuing delay nonlinearly as ρ approaches 1. The agent must distinguish between **throughput bottlenecks** (saturation) and **tail latency bottlenecks** (variance under moderate load).

## 3. Required Observations

Before declaring a bottleneck, the agent must collect:

| Observation | Why | How |
|-------------|-----|-----|
| Per-resource utilization | Identify which resource is at/near saturation | Sample GPU SM occupancy, HBM bandwidth counters, NVMe queue depth, CPU utilization |
| Latency breakdown by phase | Quantify each Amdahl term | Instrument prefill time, decode time, KV transfer time, queue wait time |
| Operational intensity | Classify compute-vs-bandwidth regime on roofline | Measure FLOPs executed and bytes moved per operation |
| Throughput vs. offered load | Find saturation knee and maximum capacity | Sweep request rate, measure sustained throughput |
| Concurrency at saturation | Identify if batch size or queue depth is the limit | Count in-flight requests/operations when throughput plateaus |
| p50/p99/p999 latency | Distinguish mean-bottleneck from tail-bottleneck | Collect latency histograms, not just means |
| Resource utilization during stalls | Catch hidden coordination bottlenecks | Correlate GPU idle time with lock contention, GC pauses, synchronization |
| Throughput under resource removal | Causal test—does removing suspected non-bottleneck change throughput? | Disable or throttle non-suspected resources; if throughput unchanged, they're not limiting |
| Sequential fraction | Bound Amdahl's ceiling on parallelization | Profile critical path for inherently serial dependencies |
| Cross-request interference | Detect shared-resource contention | Measure per-request latency as batch size varies |

## 4. Alternatives (with prefer_when / avoid_when)

### 4.1 Roofline Classification

**Approach**: Compute operational intensity of the target kernel/pipeline, plot against device roofline, classify as compute-bound or bandwidth-bound.

- **prefer_when**: The workload is a single dominant kernel (attention, MLP) whose performance maps cleanly to arithmetic intensity; you need a quick first-pass classification before deeper investigation; the system has clear hardware ceilings (GPU, HBM, PCIe)
- **avoid_when**: The bottleneck is coordination overhead (locks, scheduling, cache coherence) that doesn't appear in hardware utilization counters; the workload is a heterogeneous pipeline where no single kernel dominates; software inefficiencies (redundant copies, poor batching) create artificial ceilings below hardware limits

### 4.2 Amdahl Decomposition

**Approach**: Break total latency into sequential phases, measure each, compute improvement ceiling for each phase.

- **prefer_when**: The system has a clear pipeline structure (prefill → transfer → decode); you need to prioritize which phase to optimize first; stakeholders need quantitative justification for where to invest engineering effort
- **avoid_when**: Phases overlap significantly (pipelined execution makes wall-clock ≠ sum of phases); the bottleneck is within a single phase (need finer decomposition); interactions between phases mean fixing one shifts load to another unpredictably

### 4.3 Little's Law Capacity Analysis

**Approach**: Measure concurrency and latency, derive throughput ceiling. Identify whether concurrency or latency is the binding constraint.

- **prefer_when**: The system is queuing-theory shaped (requests arrive, wait, get served); you suspect the limit is either batch size (concurrency cap) or per-request latency; memory limits batch size and you want to quantify the throughput cost
- **avoid_when**: The system has complex feedback loops (admission control changes arrival rate based on load); service times are highly variable making mean W misleading; the system is not in steady state (ramp-up, bursty arrivals)

### 4.4 Paired Causal Experiments

**Approach**: Change exactly one variable (add bandwidth, remove contention, increase batch) while holding all else constant. Measure before/after. Attribute causation only to the changed variable.

- **prefer_when**: Multiple plausible bottlenecks exist and profiling data is ambiguous; you need to distinguish causation from correlation; the system is complex enough that analytical models (roofline, Amdahl) don't capture all interactions
- **avoid_when**: The system cannot be safely perturbed (production with no staging); running paired experiments takes prohibitively long; the bottleneck is obvious from first-principles analysis (e.g., OI = 0.5 on a device with ridge point at 100)

### 4.5 Saturation Curve Sweep

**Approach**: Gradually increase load until throughput plateaus or latency explodes. The knee identifies the bottleneck resource.

- **prefer_when**: You need to find the system's maximum capacity and the resource that gates it; you want to characterize behavior under overload; the bottleneck may be different at low load vs. high load
- **avoid_when**: The system has adaptive behavior that changes under load (e.g., dynamic batching, admission control, autoscaling) making the sweep non-monotonic; you need to diagnose a latency problem at moderate load—saturation sweeps find throughput ceilings, not tail-latency causes

### 4.6 Variance/Tail Analysis (Kingman-style)

**Approach**: Measure arrival and service time distributions, compute queuing delay contribution to tail latency.

- **prefer_when**: p99 is much worse than p50 despite moderate utilization; SLO violations are intermittent; you suspect variance (bursty arrivals, variable request sizes) rather than saturation causes the problem
- **avoid_when**: The system is throughput-bound (saturated regardless of variance); latency variance comes from a deterministic source (a single long prefill blocking all decodes); you need steady-state capacity, not tail behavior

## 5. Coupled Constraints

| This Decision | Interacts With | Because |
|---------------|----------------|---------|
| Roofline classification | tier-policy-and-eviction | A bandwidth-bound decode phase means eviction policies must minimize bytes loaded, not recompute cost |
| Amdahl decomposition | data-movement-concurrency | The KV transfer phase fraction determines how much pipeline optimization matters—if it's 5% of total time, no amount of transfer speedup matters |
| Little's Law capacity | cache-value-and-recompute | The batch-size limit (L_max) determines how many sequences compete for cache; increasing reuse (fewer evictions) increases effective concurrency |
| Paired experiments | workload-to-storage-io | Experiment validity requires understanding which workload controls generate which IO patterns—otherwise the experiment changes the wrong variable |
| Saturation curve | distributed-kv-ownership | The bottleneck may be placement—remote KV fetch latency only appears under specific placement configurations |
| Tail analysis | correctness-and-recovery | Tail latency spikes may be crash-recovery events (restoring KV from SSD), not normal-path inefficiency |

**Critical coupling**: A roofline analysis that shows "bandwidth-bound" does NOT specify which bandwidth (HBM, PCIe, NVMe, network). The agent must chain roofline → identify which memory tier is supplying the bottleneck bandwidth → evaluate whether tier policy or data movement is the lever.

## 6. Failure Modes

### 6.1 Optimizing a Non-Bottleneck
**Symptom**: Engineering effort yields no throughput or latency improvement.
**Cause**: The optimized component was not on the critical path (Amdahl fraction < 5%).
**Detection**: Measure total system throughput before/after the change. If < 5% improvement despite > 50% component improvement, the component was not the bottleneck.
**Example**: Compressing KV blocks for faster NVMe writes when the system is actually GPU-compute-bound during prefill.

### 6.2 Shifting the Bottleneck Blindly
**Symptom**: Fixing bottleneck A reveals worse bottleneck B, with B harder or more expensive to fix.
**Cause**: No Amdahl decomposition was done to predict the next bottleneck in sequence.
**Detection**: Before optimizing A, compute 1/(1-fraction_A) to see the new ceiling, and measure B's current utilization.
**Example**: Accelerating KV transfer 10× only to discover decode is now blocked on GPU memory bandwidth for the larger batches that faster transfer enables.

### 6.3 Mistaking Correlation for Causation
**Symptom**: "Fix" works once but fails to reproduce, or regresses other workloads.
**Cause**: The observed metric correlated with the bottleneck but was not causal. 
**Detection**: Paired experiment with proper controls. Change only the suspected variable.
**Example**: Observing high NVMe queue depth during latency spikes and throttling NVMe usage—when the actual cause was a scheduler bug creating bursty IO patterns.

### 6.4 Measurement Artifact (Coordinated Omission)
**Symptom**: Reported latency looks good (low p99) despite user-observed delays.
**Cause**: The measurement system pauses during stalls, undercounting requests that would have arrived during the stall.
**Detection**: Compare measured throughput × measured_latency against Little's Law prediction. If L_measured > λ_measured × W_measured, events are being lost.
**Example**: Load generator that waits for response before sending next request—it cannot detect queuing delay because it never queues.

### 6.5 Confounding Phase Interactions
**Symptom**: A component shows high utilization but throttling it doesn't improve the target metric.
**Cause**: The component serves both critical-path and non-critical-path work; high utilization is from non-critical work.
**Detection**: Tag IO operations by request priority (decode-serving vs. background writeback). Measure utilization per class.
**Example**: NVMe at 95% utilization—but 80% is background eviction writes that don't affect latency; the 15% critical reads never wait.

### 6.6 Steady-State Fallacy
**Symptom**: System performs well under sustained load but has unacceptable cold-start or burst behavior.
**Cause**: Analysis assumed steady-state but real workload is bursty (Kingman's C_a² >> 0).
**Detection**: Measure performance during load transitions (idle→peak, between prompt bursts), not just at stable offered load.

## 7. Hypotheses the Agent Can Generate

Given the observations from Section 3, the agent can form and test these hypothesis templates:

1. "Decode attention is memory-bandwidth-bound because OI = {measured_value} < {ridge_point}. Reducing KV bytes accessed (via eviction, compression, or quantization) will improve decode throughput proportionally to byte reduction" (test: compute OI, compare achieved FLOP/s to bandwidth×OI prediction).

2. "KV transfer between prefill and decode nodes is {X}% of TTFT. Amdahl ceiling for eliminating transfer entirely is {1/(1-X/100)}×. This justifies/does-not-justify engineering investment in transfer optimization" (test: measure TTFT breakdown; set transfer time to ~0 by colocating, measure improvement).

3. "The system is concurrency-limited at L_max = {batch_size}. By Little's Law, throughput ceiling is L_max / W = {value}. Increasing batch size by {delta} (via memory compression/eviction) should yield {predicted_throughput} improvement" (test: increase memory available for batching, measure throughput change).

4. "Tail latency at p99 is caused by {suspected_source} (prefill-decode interference / GC pause / remote fetch timeout), not by steady-state saturation. Eliminating this source will reduce p99 by >{predicted_reduction}% without affecting p50" (test: isolate the suspected source, compare p99 before/after).

5. "The current bottleneck is software coordination (lock contention at {location}), not hardware saturation. Hardware utilization during tail-latency events is only {X}%. Removing the coordination point will increase throughput by up to {max_idle_fraction}" (test: profile lock hold times, implement lock-free alternative, measure throughput change).

6. "Two bottlenecks interact: {bottleneck_A} at {utilization_A}% and {bottleneck_B} at {utilization_B}%. Fixing A alone yields only {limited_gain} because B immediately saturates. Must fix both, starting with A because it has the larger Amdahl fraction" (test: fix A, verify B becomes the new ceiling at the predicted level).

7. "The system appears IO-bound but is actually coordination-bound: {measured_bandwidth} is only {fraction}% of {device_peak}. Software overhead (copy, serialization, scheduling) is consuming {overhead_ms} per operation" (test: bypass software path with direct DMA, measure bandwidth increase).

8. "Variance in KV block sizes (CoV = {measured}) causes queuing delay amplification under moderate load. Normalizing block sizes to fixed geometry will reduce p99 without changing p50 or throughput" (test: pad blocks to uniform size, measure p99 change).

## 8. Experiments and Falsifiers

### 8.1 Roofline Validation
**Hypothesis**: The target operation is bandwidth-bound (OI < ridge point).
**Method**: Measure achieved FLOPs and bytes moved for the operation. Compute OI. Compare achieved performance to min(peak_compute, peak_bandwidth × OI). If actual ≈ bandwidth × OI, bandwidth-bound is confirmed.
**Falsifier**: If actual << bandwidth × OI, the operation is not reaching the bandwidth ceiling—software overhead or latency (not bandwidth) is the constraint. Look for serialization, unnecessary copies, or insufficient concurrency.

### 8.2 Amdahl Phase Isolation
**Hypothesis**: Phase X is the dominant latency contributor (fraction > 50%).
**Method**: Measure wall-clock time with phase X enabled vs. with phase X replaced by a no-op or pre-computed result (e.g., pre-stage KV in GPU memory to eliminate transfer time). Measure total improvement.
**Falsifier**: If removing phase X yields less than predicted improvement, either the phases overlap (pipeline hides X's latency) or measurement is wrong (X's time was misattributed). Verify with independent wall-clock measurement of each phase in isolation.

### 8.3 Concurrency Saturation (Little's Law)
**Hypothesis**: Throughput is limited by concurrency cap L_max, not by per-request latency W.
**Method**: Hold L constant at current max, reduce W (e.g., use shorter sequences or skip compute). If throughput increases proportionally to W reduction, W was the constraint. If throughput stays flat, L is the constraint.
**Falsifier**: If reducing W increases throughput, the system was latency-bound, not concurrency-bound. L has slack; the agent should focus on reducing W (per-request work) not increasing L (batch size).

### 8.4 Paired Bottleneck Experiment
**Hypothesis**: Resource R is the bottleneck.
**Method**: Two runs with identical workload. Run A: normal. Run B: provision 2× of resource R (add bandwidth, double memory, add GPU). If throughput(B) > throughput(A), R was limiting.
**Falsifier**: If throughput(B) ≈ throughput(A), R was not the bottleneck—the system hit a different ceiling. Try doubling a different resource.
**Control**: Only change one resource at a time. Log utilization of all resources in both runs to identify what saturates after R is relieved.

### 8.5 Interference Isolation
**Hypothesis**: Prefill-decode co-location causes decode latency degradation via resource contention.
**Method**: Measure decode TPOT with and without concurrent prefill activity on the same GPU/node. Hold batch size constant.
**Falsifier**: If TPOT is identical with and without concurrent prefill, interference is not the mechanism—something else causes the latency (e.g., batch size variation, scheduling delay).

### 8.6 Tail Latency Attribution
**Hypothesis**: p99 latency is caused by event E (GC, remote miss, scheduling stall), not by request characteristics.
**Method**: Correlate p99 latency events with occurrence of event E. Compute conditional probability: P(p99_violation | E_occurred) vs. P(p99_violation | E_absent). If strong correlation, suppress E and verify p99 improves.
**Falsifier**: If suppressing E doesn't improve p99, correlation was spurious (E and the real cause share a trigger). Instrument deeper.

### 8.7 Coordinated Omission Check
**Hypothesis**: Measured latencies are accurate (no omission bias).
**Method**: Run open-loop load generator at fixed rate λ. Count responses in window T. Verify: responses ≈ λ × T. If responses << λ × T, some requests were dropped or delayed beyond measurement window. Cross-check: L (in-flight) should equal λ × W_measured at steady state.
**Falsifier**: If response count matches but Little's Law disagrees (L ≠ λ × W), the measurement system has internal buffering that hides queuing.

### 8.8 Bandwidth vs. Latency Discrimination
**Hypothesis**: The IO path is bandwidth-limited (device saturated), not latency-limited (each op is slow but device is idle between ops).
**Method**: Measure device utilization during peak throughput. If utilization > 85%, bandwidth-limited. If utilization < 50% but throughput is low, latency-limited (insufficient concurrency or op overhead).
**Falsifier**: If increasing queue depth improves throughput without increasing utilization, the device was latency-limited and needed more concurrency. If increasing queue depth has no effect and utilization is already high, bandwidth-limited is confirmed.

## 9. Production Evidence

### 9.1 Mooncake — Proving Transfer is the Bottleneck
**System**: Mooncake (Kimi/Moonshot AI, production serving at scale)
**Problem**: Initial system appeared GPU-compute-bound (prefill GPUs at high utilization). But Amdahl decomposition revealed that KV cache transfer between disaggregated prefill and decode clusters consumed enough of the critical path to gate throughput.
**Mechanism**: By instrumenting the prefill→decode KV transfer pipeline and applying Little's Law (concurrent_transfers × transfer_latency = transfer_throughput), they identified that the KV-centric scheduler was the true bottleneck—not raw network bandwidth. Prediction-based early rejection provided the causal test: rejecting requests before KV transfer (reducing λ) improved served-request quality proportionally, confirming the transfer pipeline was saturated.
**Result**: After relieving the transfer bottleneck through CPU/DRAM/SSD caching of KV (increasing effective L_max), achieved 525% throughput increase in overload scenarios; 75% more requests in production.
**Lesson**: Profiling GPU utilization alone missed the bottleneck—it was between GPUs, not within them. Little's Law on the inter-cluster channel diagnosed it correctly.
**Source**: Qin et al., "Mooncake: A KVCache-Centric Disaggregated Architecture for LLM Serving," FAST 2025.

### 9.2 DistServe — Amdahl Applied to Phase Disaggregation
**System**: DistServe (disaggregated prefill/decode serving)
**Problem**: Colocated prefill and decode competed for GPU resources. Conventional profiling showed both phases at high utilization—ambiguous which to optimize.
**Mechanism**: Applied Amdahl's Law per-phase: measured TTFT and TPOT contributions independently. Found that neither phase was individually dominant—the interference between them (shared memory bandwidth, scheduling delay) was the bottleneck. Eliminating interference by physical separation and co-optimizing per-phase parallelism unlocked gains impossible with colocated optimization.
**Result**: 7.4× more requests served; 12.6× tighter SLO achievable while maintaining >90% SLO attainment.
**Lesson**: When Amdahl decomposition shows no single dominant phase, the bottleneck may be the interaction between phases, not any phase itself. The paired experiment (colocated vs. disaggregated) causally proved interference was the mechanism.
**Source**: Zhong et al., "DistServe: Disaggregating Prefill and Decoding for Goodput-optimized Large Language Model Serving," OSDI 2024.

### 9.3 vLLM — Little's Law Reveals Memory as Throughput Gate
**System**: vLLM (PagedAttention serving engine)
**Problem**: Throughput plateaued well below GPU compute capacity. GPU SMs showed only moderate utilization during serving.
**Mechanism**: Applied Little's Law: λ_max = L_max / W. Since W (per-step decode time) was near-optimal, L_max (batch size) was the binding constraint. L_max was gated by KV cache memory waste—fragmentation and duplication consumed 60-80% of available memory, artificially capping batch size. Paged memory management eliminated waste, increasing L_max and therefore λ_max.
**Result**: 2-4× throughput improvement, larger gains with longer sequences (where memory waste was worst). Near-zero KV memory waste.
**Lesson**: The bottleneck was not compute, bandwidth, or IO—it was an artificial concurrency cap imposed by memory management inefficiency. Little's Law correctly identified batch size as the lever; roofline analysis alone would have missed this because the GPU wasn't near any hardware ceiling.
**Source**: Kwon et al., "Efficient Memory Management for Large Language Model Serving with PagedAttention," SOSP 2023.

### 9.4 SGLang — Proving Recompute is the Bottleneck
**System**: SGLang with RadixAttention (multi-turn and structured-output serving)
**Problem**: Multi-turn conversations and tree-structured workloads (few-shot, chain-of-thought) were slow despite adequate GPU capacity.
**Mechanism**: Amdahl analysis revealed that redundant prefix computation dominated request latency—the same shared prefix was recomputed for every branch/turn. The serial fraction was not inherent but artificial (caused by cache eviction of shared prefixes). RadixAttention implemented a radix-tree-based KV cache that preserved shared prefixes, eliminating the recompute bottleneck.
**Result**: Up to 6.4× higher throughput compared to state-of-the-art systems across diverse workloads (agent control, few-shot, multi-turn chat).
**Lesson**: The "bottleneck" was not a hardware resource at saturation—it was wasted work (recomputing already-computed KV). Measurement of compute work per unique token (vs. per total token) revealed the redundancy. The causal test: with prefix sharing enabled, throughput scaled with unique tokens rather than total tokens.
**Source**: Zheng et al., "SGLang: Efficient Execution of Structured Language Model Programs," arXiv 2023 (deployed in production).

### 9.5 InfiniGen — Roofline Reclassification Through Selective Fetch
**System**: InfiniGen (offloaded KV cache inference)
**Problem**: Offloading KV to host memory made decode latency IO-bound (PCIe bandwidth-limited) instead of compute-bound.
**Mechanism**: Roofline analysis showed OI was extremely low (every attention step loaded full KV from host). By fetching only important tokens (speculative prefetch based on layer N activations), effective OI increased dramatically—fewer bytes loaded for the same FLOPs. This reclassified the operation from bandwidth-bound back to compute-bound.
**Result**: 3× improvement over prior offloading methods while maintaining model accuracy (unlike eviction approaches that sacrifice information).
**Lesson**: The roofline ridge point is not fixed—it can be shifted by changing the numerator (same compute) or denominator (fewer bytes) of operational intensity. Selective retrieval increases effective OI without changing the hardware.
**Source**: Lee et al., "InfiniGen: Efficient Generative Inference of Large Language Models with Dynamic KV Cache Management," OSDI 2024.

### 9.6 FlexGen — Little's Law Across a 3-Tier Hierarchy
**System**: FlexGen (single-GPU inference with CPU/SSD offloading)
**Problem**: Naive offloading of OPT-175B was latency-bound—each layer loaded sequentially from disk, leaving GPU idle >90% of the time.
**Mechanism**: Applied Little's Law across the 3-tier pipeline: to keep GPU busy, need L operations in-flight across GPU/CPU/disk simultaneously. Solved an LP to find the batch size that fills the bandwidth-delay product of each tier. Effective batch size of 144 was required to keep all three tiers simultaneously active.
**Result**: Achieved 1 token/s for OPT-175B on a single 16GB GPU—previously required multi-GPU. The LP solver found the exact batch size that saturated the slowest tier (disk) without exceeding memory of the fastest tier (GPU).
**Lesson**: Little's Law applies per tier: each tier needs `BDP_tier / block_size` operations in-flight to saturate. The system bottleneck is the tier with the largest BDP relative to available concurrency. The LP formulation automates bottleneck discovery across a multi-tier hierarchy.
**Source**: Sheng et al., "FlexGen: High-Throughput Generative Inference of Large Language Models with a Single GPU," ICML 2023.

### 9.7 CacheGen — Compression as Bottleneck Shifter
**System**: CacheGen (KV cache compression and streaming)
**Problem**: Network bandwidth was the diagnosed bottleneck for KV cache reuse across contexts—transferring full FP16 KV made reuse slower than recompute.
**Mechanism**: Confirmed bottleneck via paired experiment: measured reuse latency with and without transfer (local cache hit = no transfer). The difference (pure transfer time) exceeded recompute time for contexts > 1K tokens. Compression shifted the bottleneck from network to codec compute, with adaptive level selection keeping codec time below the new (lower) transfer time.
**Result**: 3.5-4.3× compression; 3.2-3.7× total delay reduction (including encode + decode overhead). Critical insight: codec overhead was a new bottleneck for very short transfers, so adaptive compression backed off for small caches.
**Lesson**: Fixing one bottleneck (network bandwidth via compression) immediately creates a new one (codec compute). The adaptive level selection is itself a real-time bottleneck detection mechanism—it probes whether codec or bandwidth is limiting and adjusts per-chunk.
**Source**: Liu et al., "CacheGen: KV Cache Compression and Streaming for Fast Large Language Model Serving," SIGCOMM 2024.

### 9.8 ARC/TinyLFU — When Eviction Policy is the Bottleneck
**System**: ARC (Megiddo & Modha, FAST 2003) and TinyLFU (Einziger et al., 2017)
**Problem**: Cache hit rate plateaus despite available memory—the eviction policy itself becomes the bottleneck by failing to predict future accesses accurately.
**Mechanism**: ARC's self-tuning between recency and frequency, and TinyLFU's admission filter, both recognized that the policy computation (not memory, not bandwidth) was gating hit rate. Measurement methodology: compare actual hit rate against Bélády's optimal (offline) algorithm. The gap between actual and optimal quantifies how much "bottleneck" remains in the policy vs. in memory capacity.
**Result**: ARC achieved within 1-5% of optimal for most workloads without parameter tuning. TinyLFU combined with W-TinyLFU (windowed) approached optimal for mixed workloads with tiny metadata overhead (<8 bits per item).
**Lesson**: When memory is adequate but hit rate is poor, the bottleneck is the eviction algorithm's prediction quality—not a hardware resource. The falsifier is the gap to Bélády's optimal: if the gap is small, more memory (not a better algorithm) is needed; if the gap is large, the algorithm is the bottleneck.
**Source**: Megiddo & Modha, "ARC: A Self-Tuning, Low Overhead Replacement Cache," FAST 2003; Einziger et al., "TinyLFU: A Highly Efficient Cache Admission Policy," ACM ToS 2017.

## 10. Implications for KV Block Storage

The bottleneck-and-measurement framework directly governs where a KV block storage system should invest optimization effort:

1. **Decode is memory-bandwidth-bound by definition** (OI ≈ 0.5-1.0 FLOP/byte). Any storage optimization that reduces bytes delivered to the attention kernel (compression, selective loading, smaller block granularity) directly improves decode throughput. Optimizations that reduce compute overhead (faster scheduling, lighter metadata) have near-zero impact on decode itself.

2. **Prefill-to-decode KV transfer is often the hidden dominant term** (Mooncake, DistServe evidence). A storage system that minimizes transfer latency (local caching, RDMA-enabled placement, predictive pre-staging) addresses the largest Amdahl fraction in disaggregated architectures.

3. **Batch size (concurrency) is frequently the true throughput gate** (vLLM evidence). A storage system that minimizes per-block memory overhead (tight packing, deduplication, compression in memory) increases L_max in Little's Law, yielding multiplicative throughput gains even if per-request latency doesn't change.

4. **The bottleneck shifts dynamically**: under low load, latency (per-block fetch time) dominates; under high load, throughput (total bandwidth, batch size cap) dominates. A storage system must instrument both regimes and adapt its strategy (e.g., switch from latency-optimized random reads to throughput-optimized sequential scans).

5. **Paired experiments are the only valid optimization signal**: before implementing any storage optimization (new eviction policy, compression codec, placement strategy), the agent must design an experiment that isolates the claimed bottleneck. The storage system should expose hooks for these experiments (injectable delays, bandwidth throttles, mock tiers) as first-class capabilities.

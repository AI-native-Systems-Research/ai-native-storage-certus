---
id: correctness-and-recovery
decision: Whether a proposed storage optimization preserves inference correctness and crash-safe durability of KV blocks
answers:
  - What invariants must an optimization never violate?
  - When is a KV block safe to serve to an attention kernel?
  - What must survive a crash and what can be lost?
  - How does the system detect and recover from stale or corrupt KV state?
  - When is it safe to reuse or reclaim a KV block?
  - What ordering guarantees does the storage layer owe the inference engine?
inputs:
  - block lifecycle state (unsealed, sealed, in-flight, reclaimable)
  - reference count and active reader/writer set per block
  - crash domain boundaries (GPU, host process, NVMe, remote)
  - checksum or content-hash per block
  - sequence-to-block mapping (which sequences depend on which blocks)
  - completion status of in-flight writes and transfers
  - model version and quantization config that produced the block
  - SLO budget for recovery (max acceptable TTFT penalty on crash)
owns: ['atomicity', 'crash recovery', 'stale completion', 'safe reuse', 'corruption']
excludes:
  - eviction policy selection (see tier-policy-and-eviction)
  - placement and routing decisions (see distributed-kv-ownership)
  - block value computation (see cache-value-and-recompute)
  - IO pipeline depth tuning (see data-movement-concurrency)
  - attention kernel internals
  - hardware specifications
related:
  - tier-policy-and-eviction
  - distributed-kv-ownership
  - data-movement-concurrency
  - kv-footprint-and-lifecycle
  - workload-to-storage-io
---

## 1. Decision Being Made

The optimization agent must decide, for every proposed change to the KV storage system:

1. **Atomicity boundary**: At what granularity must writes be all-or-nothing? A partial KV block (some layers written, others not) is worse than no block at all—it produces silently wrong attention outputs. The agent must determine whether an optimization preserves the atomic-write invariant.

2. **Crash recovery scope**: After a host process crash, NVMe power loss, or network partition, which blocks can the system trust? The agent must decide what metadata is needed to distinguish complete-and-valid blocks from partial-and-corrupt ones.

3. **Stale completion detection**: When a block's content no longer matches the inference state that produced it (model version changed, quantization differs, prefix was extended), serving it produces wrong outputs with no error signal. The agent must determine what staleness checks are necessary.

4. **Safe reuse timing**: A block can only be freed or overwritten when no active decode depends on it. The agent must decide how reference tracking works and when reclamation is safe.

5. **Corruption detection**: Silent bit corruption in KV tensors propagates through attention as silently wrong outputs. The agent must decide what verification is cost-effective versus what can rely on hardware guarantees.

These are not performance decisions—they are correctness gates. An optimization that violates any of these invariants is invalid regardless of its throughput benefit.


## 2. Mental Model and Equations

### 2.1 Block Validity Predicate

A stored KV block `b` is safe to serve if and only if:

```
valid(b) = complete(b) ∧ fresh(b) ∧ uncorrupted(b) ∧ reachable(b)
```

Where:
- `complete(b)` = all L layers × H heads of the block have been written and persisted
- `fresh(b)` = the model config (version, quantization, head layout) that produced `b` matches the currently serving config
- `uncorrupted(b)` = stored bytes match what was originally written (checksum passes)
- `reachable(b)` = the block's metadata entry exists and points to valid physical storage

Serving a block where `¬valid(b)` produces **silent output corruption**—the attention kernel computes wrong values with no exception or error signal.

### 2.2 Atomicity Invariant

For a block spanning L layers, each with tensor size S_l:

```
atomic(b) ⟺ ∀ l ∈ [1, L]: persisted(b, l) = true  ∨  ∀ l ∈ [1, L]: persisted(b, l) = false
```

No intermediate state (some layers persisted, others not) may be visible to readers. This requires either:
- **Write-ahead sealing**: Write all layers, then atomically set a "sealed" flag. Readers check sealed before serving.
- **Append-only with length check**: Store layers sequentially; readers verify total size equals expected `L × S_l` before serving.
- **Copy-on-write**: Write to shadow location; atomically swap pointer on completion.

### 2.3 Reference Safety

A block `b` may be reclaimed only when:

```
safe_to_reclaim(b) = (refcount(b) == 0) ∧ (¬in_flight_read(b)) ∧ (¬in_flight_write(b))
```

Where:
- `refcount(b)` = number of active sequences whose attention depends on `b`
- `in_flight_read(b)` = a DMA or IO read is currently transferring `b` to a consumer
- `in_flight_write(b)` = the block is still being written (not yet sealed)

Violating this produces use-after-free: the attention kernel reads freed memory (garbage values) or partially overwritten data.

### 2.4 Crash Recovery Bound

After crash, recovery time is bounded by:

```
T_recovery = T_scan + T_validate + T_rebuild_index
```

Where:
- `T_scan` = time to enumerate persisted blocks (proportional to total block count)
- `T_validate` = time to verify each block's completeness and checksum
- `T_rebuild_index` = time to reconstruct the prefix→block lookup from validated blocks

For a system with `B` blocks, each of size `S`:
```
T_recovery ≈ B × (S / bandwidth_checksum + C_index_insert)
```

The SLO constraint: `T_recovery < T_ttft_budget` for the first request after crash. If violated, the system must either reduce B (smaller tier), use incremental validation, or accept serving with unvalidated blocks (trading correctness risk for availability).

### 2.5 Staleness Detection Cost

For each serve operation, staleness verification costs:

```
C_freshness_check = C_hash_compare + C_config_compare
```

Where:
- `C_hash_compare` = comparing stored content hash against expected prefix hash (O(1) metadata lookup)
- `C_config_compare` = comparing block's model_version field against current serving config (O(1))

This cost is negligible per-serve (nanoseconds) but the consequence of skipping it is unbounded (silent corruption of all subsequent tokens in the sequence).


## 3. Required Observations

Before deciding whether an optimization preserves correctness:

| Observation | Why Needed | Source |
|---|---|---|
| Block seal rate vs crash rate | If crashes are rare relative to seals, the incomplete-block window is small | Process crash logs, seal completion counters |
| In-flight write duration distribution | Bounds the vulnerability window for partial writes | Write-path latency histogram |
| Reference count distribution | Determines how long blocks remain un-reclaimable | Per-block refcount histogram |
| Checksum verification overhead | Whether integrity checks fit within latency budget | Benchmarked checksum throughput |
| Model version change frequency | How often freshness invalidation occurs | Deployment log, config change events |
| Blocks surviving crash (validated vs total) | Recovery efficiency — high ratio means low overhead | Post-crash scan results |
| Cross-crash-domain transfer rate | Blocks in-flight across crash domains are at highest risk | Transfer counters at domain boundaries |
| Silent corruption rate (detected via checksums) | Whether integrity checking is justified by actual error rate | ECC error logs, checksum failure counters |
| Concurrent readers per block at peak | Determines whether reclamation races are plausible | Max concurrent refcount per block |
| Time between last write and first read of new blocks | Whether read-before-seal races can occur | Write-completion to first-read latency |
| Stale block serve rate (detected or estimated) | Whether staleness is a real problem or theoretical | Config-mismatch detection counters |


## 4. Alternatives with Prefer/Avoid

### 4.1 Write-Ahead Seal Bit (WAL-style)

- **Mechanism**: Each block has a metadata seal bit stored in a separate, durable location. Writer sets seal=false before writing tensor data, writes all layers, then sets seal=true. Readers refuse blocks with seal=false. On crash recovery, blocks with seal=false are discarded.
- **Prefer when**: Storage medium supports fast metadata updates independent of data writes (NVMe with separate metadata region); crash recovery must be fast (only scan seal bits, not full tensors); write path can tolerate one extra sync for the seal flip.
- **Avoid when**: Seal-bit storage itself is in the same failure domain as the data (both lost on crash defeats the purpose); write path is latency-critical and cannot absorb the extra sync (sub-millisecond write SLO); block writes are small enough that atomic-write hardware guarantees suffice.

### 4.2 Content-Hash Verification (Self-Describing Blocks)

- **Mechanism**: Each block stores a cryptographic or fast hash (xxHash, CRC32C) of its tensor data alongside the data. On read, the hash is recomputed and compared. Mismatch → block is invalid, trigger recompute. Prefix blocks use the token-sequence hash as their identity—content-addressing makes corruption detectable by definition.
- **Prefer when**: Silent corruption is a real risk (SSD bit-rot, network corruption during transfer, multi-bit errors beyond ECC coverage); blocks are transferred across crash domains (network, disk); the system serves long enough that accumulated bit-rot probability is non-negligible.
- **Avoid when**: All storage is in ECC-protected DRAM with short lifetime (< minutes); checksum computation overhead exceeds the latency budget for the hot path; blocks are regenerated so frequently that corruption would be overwritten before detection.

### 4.3 Reference Counting with Epoch-Based Reclamation

- **Mechanism**: Each block maintains a reference count incremented on sequence association and decremented on sequence completion. Reclamation is deferred to epoch boundaries (e.g., between scheduling iterations) when no active computation can hold a stale reference. vLLM's BlockManager uses this pattern (SOSP 2023).
- **Prefer when**: Multiple sequences share prefix blocks (parallel sampling, beam search, system-prompt sharing); block lifetime is long relative to sequence lifetime; the scheduling loop has natural quiescent points between iterations.
- **Avoid when**: The system is purely single-sequence-per-block (no sharing, refcount is always 0 or 1—simpler ownership suffices); scheduling is continuous with no natural epoch boundaries; reference counting overhead (atomic operations per association) is measurable relative to block access time.

### 4.4 Copy-on-Write for Shared Blocks

- **Mechanism**: When a shared prefix block must be extended (beam search diverges, parallel samples fork), the system copies the block rather than appending in-place. The original remains valid for other sharers. Only the copy is mutated. vLLM uses this for beam search (Kwon et al., SOSP 2023).
- **Prefer when**: Prefix blocks are shared across sequences that will diverge; in-place mutation would corrupt other sequences' view; memory is sufficient to hold the copy (block is smaller than the wasted-recompute cost of the alternative).
- **Avoid when**: Blocks are never shared (single-sequence ownership); memory pressure is extreme and copies would trigger cascading evictions; the divergence point is predictable enough to pre-split blocks before sharing begins.

### 4.5 Idempotent Writes with Deduplication

- **Mechanism**: Write operations are idempotent—writing the same block content to the same address is a no-op. Block identity is content-hash, so duplicate writes are detected and deduplicated. Retries after partial failure are safe because re-writing produces the same result. (LMCache's content-addressed store design)
- **Prefer when**: Write failures are common (unreliable network, remote storage); retries must be cheap and safe; multiple writers may independently compute the same prefix (distributed prefill); simplicity of "just write again" outweighs dedup overhead.
- **Avoid when**: Block identity is position-based rather than content-based (e.g., sequence-specific decode blocks where content varies per sequence); dedup lookup cost exceeds the cost of occasionally storing a duplicate; writes are always to local memory with reliable completion.

### 4.6 Lazy Validation (Serve-Then-Verify)

- **Mechanism**: Serve blocks immediately without upfront validation. Run background checksums and freshness checks asynchronously. If a served block is later found invalid, mark dependent output as potentially corrupted and optionally trigger regeneration. Trades correctness risk for latency reduction.
- **Prefer when**: Corruption rate is empirically near-zero (ECC-protected memory, short block lifetime); TTFT SLO is extremely tight and cannot absorb validation latency; the downstream application tolerates occasional quality regression (non-safety-critical inference).
- **Avoid when**: Output correctness is safety-critical; corruption rate is non-negligible; blocks traverse unreliable media (network, SSD without end-to-end checksums); there is no mechanism to detect and surface post-hoc quality regression.


## 5. Coupled Constraints

### 5.1 Atomicity ↔ Write Throughput
Stronger atomicity guarantees (full block fsync before sealing) reduce write throughput. A 128-layer block at 4096 bytes/token/layer for a 4K-token block is ~2 GB. Synchronous writes at NVMe sequential bandwidth (~6 GB/s) take ~330ms per block. Async writes with deferred sealing pipeline multiple blocks but expand the crash-vulnerability window.

### 5.2 Checksum Overhead ↔ Serve Latency
Full-block checksum verification on the read path adds latency proportional to block size. For a 2 GB block at 30 GB/s memory bandwidth, verification takes ~67ms. This may exceed the entire TTFT budget. Incremental checksums (per-layer granularity) reduce verification to only the required layers but add storage overhead.

### 5.3 Reference Counting ↔ Scheduling Freedom
Strict reference counting prevents the scheduler from reclaiming blocks that active sequences depend on. Under memory pressure, this creates a tension: the scheduler wants to evict low-value blocks, but high refcount blocks (popular shared prefixes) are exactly the ones consuming the most memory and cannot be reclaimed.

### 5.4 Recovery Time ↔ Tier Capacity
Larger storage tiers (more blocks on NVMe/SSD) increase recovery time linearly. A system with 10,000 blocks at 2 GB each needs 20 TB of validation throughput. Even at NVMe read bandwidth (~6 GB/s), full validation takes >55 minutes. This forces either: smaller tiers, incremental validation, or accepting unvalidated blocks post-crash.

### 5.5 Freshness ↔ Rolling Upgrades
Model version checks invalidate all cached blocks on model update. In production, rolling upgrades transition nodes incrementally—some serve old model, some new. KV blocks computed by old-model nodes are stale for new-model nodes. The tighter the freshness check, the more cache is invalidated during upgrades, causing a "cold start storm" of recomputation.

### 5.6 Safe Reuse ↔ Compression
Lossy compression (CacheGen, 3.5–4.3× compression, SIGCOMM 2024) introduces a new validity dimension: the decompressed tensor is not bit-identical to the original. A block compressed and decompressed is "approximately correct" with negligible quality impact—but standard checksum verification would flag it as corrupted. The correctness model must distinguish between bit-exact integrity (for uncompressed blocks) and statistical-quality bounds (for compressed blocks).


## 6. Failure Modes

### 6.1 Torn Write (Partial Block Persisted)

**Trigger**: Process crash or power loss during multi-layer block write. Some layers are on-disk, others are not. No seal bit or size check is performed by reader.
**Symptom**: Attention kernel reads zero-filled or garbage values for missing layers. Output is nonsensical but no error is raised—the kernel just computes on whatever bits are at that address.
**Diagnostic**: Post-crash audit finds blocks whose stored size < expected size. Per-layer checksum reveals missing or zeroed layers.
**Severity**: Silent output corruption affecting every token generated from the corrupted block.

### 6.2 Use-After-Free (Reclaimed Block Still Referenced)

**Trigger**: Block freed while a concurrent decode step still holds a pointer to it. Race condition between scheduler (freeing completed sequences' blocks) and attention kernel (reading those blocks for the current step).
**Symptom**: Attention reads stale/garbage memory. May produce wrong tokens or, in extreme cases, segfault.
**Diagnostic**: ASAN/memory sanitizer detects the access; or output quality suddenly degrades for a single sequence mid-generation without request-level errors.
**Severity**: Per-sequence corruption. In vLLM, prevented by the block manager's refcount + epoch-based free (Kwon et al., SOSP 2023). Optimization that bypasses the refcount check for "performance" reintroduces this.

### 6.3 Stale Block Served After Model Update

**Trigger**: Rolling model upgrade changes head dimensions, quantization, or layer count. Cached blocks from the old model remain in storage. No version check on serve. Block is fetched and used by the new model's attention kernel.
**Symptom**: Dimension mismatch may cause runtime crash (lucky case) or, if dimensions happen to match, silent output quality degradation (unlucky case). CacheGen notes that even minor distributional drift makes cached KV unsuitable (SIGCOMM 2024).
**Diagnostic**: A/B comparison of output quality with cache enabled vs disabled during upgrade window.
**Severity**: Cluster-wide output quality degradation during and after upgrade, potentially undetected for hours.

### 6.4 Phantom Completion (Index Points to Garbage)

**Trigger**: Crash occurs after the index is updated (block registered as available) but before the data write completes. On recovery, the index says the block exists but the storage location contains partial or zero data.
**Symptom**: Subsequent requests "hit" in the cache but receive corrupt data. System reports high hit rate with degraded output quality.
**Diagnostic**: Validate stored data against index entries post-crash. Hit-rate vs output-quality divergence.
**Severity**: Insidious—the system appears healthy (high hit rate, low latency) while producing corrupt output.

### 6.5 Compression Quality Drift

**Trigger**: Lossy KV compression (CacheGen-style) is applied at varying compression ratios depending on bandwidth conditions. Under pressure, ratio increases. Decompressed blocks deviate further from ground truth. Quality degrades gradually without a hard failure boundary.
**Symptom**: Perplexity increases monotonically with compression ratio. At 4× compression, impact is negligible; at 8–10×, quality visibly degrades. No threshold triggers an alert.
**Diagnostic**: Continuous monitoring of output quality metrics (perplexity on a held-out calibration set) correlated with compression ratio distribution.
**Severity**: Gradual, undetected quality erosion. Worse under load (exactly when quality matters most).

### 6.6 Cross-Domain Transfer Corruption

**Trigger**: KV block transferred between nodes (GPU→network→remote GPU) suffers bit flip not detected by transport layer. Block is stored and served without end-to-end checksum.
**Symptom**: Single corrupted tensor element propagates through attention as a subtle but persistent bias in output distribution. Extremely difficult to detect without checksums.
**Diagnostic**: End-to-end checksum at write and verify at read. Without it, only detectable via output quality regression analysis.
**Severity**: Per-block corruption, persistent until the block is evicted or overwritten.


## 7. Hypotheses the Agent Can Generate

1. **H1**: "Adding a per-block seal bit to the NVMe write path will eliminate torn-write corruption with <2% write throughput overhead, because the seal-flip is a single 4KB metadata write amortized over a ~2GB data write." (Testable: Measure write throughput with/without seal synchronization.)

2. **H2**: "Crash recovery can be reduced from full-scan to O(1) by maintaining a write-ahead log of in-progress block writes. Only WAL-listed blocks need validation on restart." (Testable: Compare recovery time with WAL vs full-scan on a system with 10K+ blocks.)

3. **H3**: "The current system has zero stale-block serves because model updates are infrequent, so freshness checks can be deferred to background rather than inline, saving per-serve latency." (Testable: Instrument stale-serve detection in shadow mode; if rate is truly zero over 7 days, inline checks are overhead without benefit.)

4. **H4**: "Reference count contention on popular shared prefix blocks is limiting scheduling throughput. Switching to epoch-based reclamation (free only at scheduling boundaries) will eliminate atomic-refcount overhead." (Testable: Profile atomic operation cost on hot blocks; measure scheduling throughput before/after.)

5. **H5**: "End-to-end checksums on the cross-node transfer path will detect the ~10^-12 undetected bit error rate of Ethernet at negligible cost (<0.1% latency increase for CRC32C on 2GB blocks at memory bandwidth)." (Testable: Benchmark CRC32C throughput vs block transfer time; inject corruption and verify detection.)

6. **H6**: "Post-crash, lazily validating blocks on first access (rather than scanning all blocks at startup) will reduce recovery time from minutes to milliseconds for the first request, because only the requested block needs validation." (Testable: Compare first-request latency under eager-scan vs lazy-validation recovery strategies.)

7. **H7**: "Copy-on-write overhead for beam search (vLLM pattern) can be eliminated for read-only prefix blocks by proving the prefix is sealed and will never be mutated, removing the copy entirely." (Testable: Track how many CoW copies are triggered for sealed vs unsealed blocks; if 100% of copies are on unsealed blocks, the sealed path can skip CoW.)


## 8. Experiments and Falsifiers

### E1: Seal-Bit Overhead Measurement
- **Setup**: Write 1000 blocks with and without post-write seal synchronization. Measure end-to-end write throughput and per-block latency.
- **Metric**: Throughput (GB/s), per-block latency (ms), seal-sync cost as % of total.
- **Falsifier for H1**: If seal sync adds >5% throughput overhead (metadata write is not dominated by data write), H1's cost claim is falsified. Investigate batched seal-flips.

### E2: WAL vs Full-Scan Recovery
- **Setup**: Populate storage with 10K blocks. Simulate crash (kill process). Recover using (a) full scan + validate all blocks, (b) WAL-based recovery validating only in-progress blocks.
- **Metric**: Time to first serve-ready state, number of blocks validated, corrupt blocks detected.
- **Falsifier for H2**: If the WAL itself is lost in the crash (same failure domain as data), recovery falls back to full scan. If WAL maintenance overhead exceeds recovery-time savings across expected crash frequency, the WAL is not justified.

### E3: Stale-Serve Rate Under Rolling Upgrade
- **Setup**: Deploy model update to 1 of 8 nodes. Route mixed traffic. Instrument freshness checks (shadow mode—log mismatches without rejecting).
- **Metric**: Stale-serve rate (blocks from old model served by new-model nodes), output quality delta.
- **Falsifier for H3**: If stale-serve rate is >0 during any upgrade window, inline freshness checks are necessary. If quality impact of stale serves is measurable (perplexity increase >0.5%), the optimization cannot defer checks.

### E4: Refcount Contention Profiling
- **Setup**: Profile atomic increment/decrement operations on blocks with high sharing (>100 concurrent references). Measure cycles consumed by refcount operations vs total scheduling loop time.
- **Metric**: % of scheduling loop time in refcount atomics, cache-line contention rate.
- **Falsifier for H4**: If refcount operations consume <1% of scheduling time, the optimization provides negligible benefit. If epoch-based reclamation increases peak memory usage beyond acceptable bounds (delayed free holds dead blocks), it creates a worse problem.

### E5: End-to-End Checksum Cost
- **Setup**: Compute CRC32C over 2GB block at memory bandwidth. Measure time relative to network transfer time for same block.
- **Metric**: Checksum time / transfer time ratio; detected corruption rate over extended operation.
- **Falsifier for H5**: If checksum time exceeds 1% of transfer time, or if the actual bit error rate is so low that no corruption is detected in 10^12 bytes transferred, the overhead may not be justified. (Counter-argument: the cost of one undetected corruption may still justify the overhead.)

### E6: Lazy vs Eager Recovery First-Request Latency
- **Setup**: Crash with 10K blocks persisted. Measure time from restart to first request served under (a) full scan first, (b) validate-on-access.
- **Metric**: First-request latency, risk of serving unvalidated blocks.
- **Falsifier for H6**: If the first request's block happens to be corrupt (detected during lazy validation), the request fails—eager scan would have caught this. If P(corrupt block) × expected requests before full background scan completes is >0, lazy validation has a nonzero corruption-serve risk.

### E7: CoW Elimination for Sealed Blocks
- **Setup**: Instrument vLLM-style block manager to log every copy-on-write trigger. Categorize by block seal status (sealed prefix vs unsealed decode).
- **Metric**: % of CoW events on sealed vs unsealed blocks; memory savings from skipping CoW on sealed blocks.
- **Falsifier for H7**: If any CoW is triggered on a "sealed" block (indicating the seal invariant is violated somewhere), the optimization is unsafe. If CoW on sealed blocks represents <5% of total CoW events, the savings are negligible.


## 9. Production Evidence

### 9.1 vLLM: Block Manager Reference Safety
- **System**: vLLM (Kwon et al., SOSP 2023)
- **Problem**: Parallel sampling and beam search share prefix KV blocks across multiple sequences. When sequences diverge or complete at different times, shared blocks must not be freed while still in use, and must not be modified in-place when other sequences depend on the original.
- **Mechanism**: Reference-counted block table with copy-on-write semantics. Each physical block maintains a refcount. Free only when refcount reaches zero. On write to shared block: allocate new physical block, copy contents, redirect writer's page table entry. Scheduling loop checks refcount before reclamation.
- **Result**: 2–4× throughput improvement over FasterTransformer (which preallocates and wastes memory) with zero correctness violations from sharing. Enables batch sizes that would be impossible without safe sharing.
- **Lesson**: Reference counting + CoW is the minimum viable correctness mechanism for shared KV blocks. Any optimization that weakens refcount guarantees (lazy free, approximate refcount) must prove equivalent safety or accept a bounded error rate.

### 9.2 Mooncake: Disaggregated KV Transfer Durability
- **System**: Mooncake (Kimi/Moonshot AI, FAST 2025)
- **Problem**: KV blocks computed on prefill nodes must be transferred to decode nodes across network. Transfer failures (timeout, partial delivery, node crash mid-transfer) leave decode nodes with incomplete KV. Serving incomplete KV produces wrong outputs.
- **Mechanism**: CachePool treats KV blocks as immutable objects transferred in full or not at all. Transfer is all-or-nothing from the decoder's perspective—incomplete transfers are not registered in the local block table. Prediction-based early rejection prevents starting transfers that would not complete within SLO.
- **Result**: 525% throughput improvement in simulated scenarios; 75% more requests in production—achieved without correctness compromises by treating transfer atomicity as non-negotiable.
- **Lesson**: In disaggregated architectures, network transfer is a crash domain boundary. The storage layer must treat cross-domain transfers with the same atomicity guarantees as local writes: visible only on successful completion, invisible on failure.

### 9.3 SGLang RadixAttention: Prefix Immutability Invariant
- **System**: SGLang (Zheng et al., 2024)
- **Problem**: Radix tree enables automatic KV reuse across requests sharing prefixes. If a "shared" prefix block is mutated (extended, partially overwritten), all requests referencing that prefix see corrupted state.
- **Mechanism**: Tree nodes representing completed prefixes are immutable. Extension creates a new child node—never modifies the parent. Garbage collection only removes nodes with zero references and no children. The tree structure itself encodes the immutability invariant: a node with children cannot be modified.
- **Result**: 6.4× throughput improvement while maintaining exact output equivalence to non-cached execution—no quality degradation from sharing.
- **Lesson**: Immutability is the strongest correctness guarantee for shared blocks. Once sealed, a prefix block's content is its identity (content-addressed). Any system offering prefix sharing must enforce immutability at the storage level, not merely by convention.

### 9.4 CacheGen: Lossy Compression Correctness Boundary
- **System**: CacheGen (Liu et al., SIGCOMM 2024)
- **Problem**: Compressing KV blocks 3.5–4.3× for network transfer is lossy—decompressed tensors are not bit-identical to originals. Must establish that approximate KV does not degrade output quality below acceptable thresholds.
- **Mechanism**: Adaptive compression exploiting KV distributional properties. Quality validated empirically: negligible impact at 3.5–4.3× compression. Fallback: when bandwidth drops further, system recomputes KV rather than increasing compression beyond the quality boundary.
- **Result**: 3.2–3.7× total delay reduction with negligible quality impact. At extreme compression (beyond 4.3×), quality degrades—the system recomputes rather than risk it.
- **Lesson**: Lossy compression introduces a new correctness dimension: statistical quality bounds rather than bit-exact integrity. The storage system must track compression ratio per block and enforce a maximum beyond which recompute is preferred. Checksum verification must account for intentional compression differences versus unintentional corruption.

### 9.5 FlexGen: SSD Offload Write Ordering
- **System**: FlexGen (Sheng et al., ICML 2023)
- **Problem**: Offloading KV to SSD via CPU DRAM creates a multi-stage pipeline (GPU→CPU→SSD). If reads are issued before writes complete (pipeline scheduling error), the attention kernel reads uninitialized data from SSD.
- **Mechanism**: Linear-programming-based scheduler computes a global execution order that provably respects write-before-read dependencies. Tensors are scheduled with explicit lifetime tracking across all three tiers. No read issued until the block's write to that tier is confirmed complete.
- **Result**: 1 token/s generation throughput for OPT-175B on a single 16GB GPU—impossible without safe offloading. The LP solver guarantees no read-before-write violations by construction.
- **Lesson**: When KV traverses multiple tiers asynchronously, write-read ordering must be explicitly enforced. Relying on "writes are fast enough" is insufficient—formal scheduling (LP, dependency graphs) prevents temporal ordering violations that would be intermittent and extremely difficult to diagnose.

### 9.6 LMCache: Engine-Independent Crash Survival
- **System**: LMCache (PyTorch Foundation ecosystem; production at CoreWeave/Cohere)
- **Problem**: When KV cache is tied to the inference engine process, a process crash (OOM kill, segfault, update restart) loses all cached KV. Recovery requires recomputing every active prefix—a cold-start storm that can take minutes for long-context workloads.
- **Mechanism**: External daemon process owns KV blocks independently of engine lifetime. Blocks are written to daemon-managed storage (CPU DRAM, SSD, or remote) with explicit durability guarantees. Engine crash → restart → reconnect to daemon → all KV still available. Content-addressed blocks enable safe reuse across engine instances.
- **Result**: Zero-downtime engine restarts with full cache preservation. Eliminates cold-start storms after crashes. Enables 10× MoE inference performance via cross-process KV sharing.
- **Lesson**: Crash recovery is fundamentally a data ownership problem. KV owned by the engine dies with the engine. KV owned by an independent storage layer survives arbitrary engine failures. The storage system is the crash domain boundary—it must outlive its clients.

### 9.7 DistServe: Transfer Atomicity in Prefill-Decode Split
- **System**: DistServe (Zhong et al., OSDI 2024)
- **Problem**: After prefill completes on a prefill node, the entire KV state must be transferred to a decode node before the first decode step. Partial transfer means the decode node has some layers but not others—producing wrong attention for the first generated token.
- **Mechanism**: KV transfer is a blocking prerequisite for decode scheduling. The scheduler does not enqueue a sequence for decode until transfer is confirmed complete. Transfer failure → re-prefill (recompute is the safe fallback for any transfer-domain failure).
- **Result**: Serves 7.4× more requests than colocated serving. Transfer atomicity is enforced by the scheduler, not the storage layer—but the principle is identical: never expose partial state to the consumer.
- **Lesson**: The "transfer complete" signal is the atomicity barrier in disaggregated systems. Any optimization that pipelines decode before full transfer completion (serving partial KV as it arrives) must prove per-layer independence—otherwise it violates the atomicity invariant.

### 9.8 InfiniGen: Selective Retrieval Correctness Bound
- **System**: InfiniGen (Lee et al., OSDI 2024)
- **Problem**: Offloading full KV to CPU memory and fetching all of it back is too slow. Fetching only a subset (important heads/layers) is faster but risks missing KV entries that the attention kernel needs—producing incorrect attention scores.
- **Mechanism**: Speculative attention-score prediction using lightweight layer inputs to estimate which KV entries will receive high attention weight. Only high-scoring entries are prefetched. Critical invariant: the prediction must be conservative—better to fetch unnecessary entries than miss necessary ones.
- **Result**: 3× performance over full-fetch offloading with "substantially better model accuracy" than prior selective approaches. The conservative prediction ensures no critical KV entries are dropped.
- **Lesson**: Selective retrieval is a correctness-performance tradeoff with a hard boundary. The storage system can serve partial blocks only if the selection oracle is provably conservative. A false negative (needed entry not fetched) causes silent output corruption. The system must default to "fetch more" when prediction confidence is low.


## 10. Implications for KV Block Storage

1. **Seal-before-serve is the fundamental invariant**: No optimization may expose a partially written block to readers. The storage layer must enforce this with a mechanism that survives crashes—content-length verification, seal bits, or write-ahead logging. This is non-negotiable.

2. **Content-addressing provides correctness by construction**: When blocks are identified by content hash (token sequence → block identity), staleness and corruption collapse into a single check: does the stored content match its claimed hash? This eliminates an entire class of freshness bugs and makes deduplication and safe reuse trivial.

3. **Reference counting is the minimum safe sharing mechanism**: Any system that shares KV blocks across sequences must track live references. Optimizations that bypass refcounting (lazy free, timer-based reclaim, "probably safe" heuristics) trade a guaranteed invariant for a probabilistic one—unacceptable for inference correctness.

4. **Crash domain boundaries require transfer atomicity**: Every crossing between crash domains (GPU↔host, host↔NVMe, node↔network↔node) is a potential partial-write site. The storage layer must treat each crossing as an atomic-transfer boundary: visible on success, invisible on failure. This principle scales from local SSD writes to multi-node RDMA transfers.

5. **Lossy compression creates a dual-integrity model**: The storage system must distinguish "bit-exact integrity" (uncompressed blocks: checksum must pass) from "quality-bounded integrity" (compressed blocks: decompressed output within acceptable statistical distance). One checksum policy cannot serve both; block metadata must declare which integrity model applies.

6. **Recovery time is bounded by index reconstruction, not data validation**: With content-addressed, sealed blocks, post-crash recovery only needs to rebuild the prefix→location index from surviving sealed blocks. Data validation (checksums) can happen lazily on first access. The critical path is index availability, not data verification.

7. **The safest fallback is always recompute**: When any correctness check fails—checksum mismatch, stale version, incomplete transfer, missing layers—the correct response is to recompute from scratch rather than attempt repair. Recompute is expensive but provably correct. The storage system should make "drop and recompute" the default failure path, with repair reserved for cases where correctness is formally provable.

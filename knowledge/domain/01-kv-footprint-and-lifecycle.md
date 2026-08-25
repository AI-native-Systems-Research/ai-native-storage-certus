I now have sufficient context from the codebase. Let me compose the full domain reference file.

```markdown
---
id: kv-footprint-and-lifecycle
decision: How to size, address, and manage the lifetime of KV cache blocks
answers:
  - What determines KV size, identity, and lifetime?
  - How large is a single KV block in bytes?
  - When should a block be sealed vs left open?
  - How are shared prefixes identified and deduplicated?
  - What namespace scheme prevents cross-model or cross-version collision?
  - When does a cached block become invalid?
inputs:
  - model architecture parameters (num_layers, num_kv_heads, head_dim, dtype)
  - block size in tokens
  - token hash or prefix tree depth
  - request arrival rate and prefix distribution
  - model version and quantization config
  - attention window size (if windowed)
owns: ['data shape', 'block geometry', 'sealing', 'sharing', 'namespaces', 'invalidation']
excludes:
  - eviction policy selection (see tier-policy-and-eviction)
  - placement and routing decisions (see distributed-kv-ownership)
  - IO concurrency and pipeline depth (see data-movement-concurrency)
  - hardware specifications
  - attention kernel internals
related:
  - tier-policy-and-eviction
  - workload-to-storage-io
  - cache-value-and-recompute
  - distributed-kv-ownership
  - correctness-and-recovery
---

## 1. Decision Being Made

The optimization agent must decide:

1. **Block geometry**: How many tokens per block? This determines object size, internal fragmentation, and the granularity of sharing and eviction.
2. **Block identity**: What hash/key scheme addresses blocks so that prefix sharing is discoverable and cross-model collisions are impossible?
3. **Sealing policy**: When is a block considered immutable (sealed) vs still accumulating tokens? Sealed blocks can be stored, shared, and deduplicated; unsealed blocks are ephemeral.
4. **Sharing scope**: Which blocks are shareable across requests, sessions, or nodes? What is the deduplication boundary?
5. **Namespace design**: How do model version, quantization level, and tenant isolate their KV address spaces?
6. **Invalidation rules**: When does a block become stale and what cascade does invalidation trigger?

These decisions are foundational—every downstream policy (eviction, placement, movement) operates on the objects defined here.

## 2. Mental Model and Equations

### Per-Token KV Size

```
kv_bytes_per_token = 2 × num_kv_heads × head_dim × dtype_bytes × num_layers
```

Where:
- Factor of 2: one K tensor + one V tensor
- `num_kv_heads`: number of KV heads (may differ from query heads in GQA/MQA)
- `head_dim`: dimension per head (typically 128)
- `dtype_bytes`: 2 for FP16/BF16, 1 for INT8/FP8
- `num_layers`: transformer layers

**Examples:**
| Model | Layers | KV Heads | Head Dim | Dtype | Per-Token KV |
|-------|--------|----------|----------|-------|--------------|
| Llama-2 7B | 32 | 32 | 128 | FP16 | 512 KB |
| Llama-2 70B | 80 | 8 (GQA) | 128 | FP16 | 320 KB |
| Llama-3 405B | 126 | 8 (GQA) | 128 | FP16 | 504 KB |
| Mixtral 8x7B | 32 | 8 (GQA) | 128 | FP16 | 128 KB |

### Block Size

```
block_bytes = block_size_tokens × kv_bytes_per_token
```

For a 16-token block on Llama-2 70B: `16 × 320 KB = 5.12 MB`
For a 16-token block on Llama-2 7B: `16 × 512 KB = 8.19 MB`

### Sequence Total Footprint

```
total_kv_bytes = seq_len × kv_bytes_per_token
num_blocks = ceil(seq_len / block_size_tokens)
```

A 4096-token sequence on Llama-2 70B: `4096 × 320 KB = 1.28 GB`

### Block Identity (Hash-Based Addressing)

```
block_id = H(namespace || token_ids[start:end] || position_offset)
```

Where H is a collision-resistant hash (e.g., xxHash128). The namespace encodes model identity and quantization. Position offset enables position-independent caching when using RoPE with offset support.

### Prefix Sharing Ratio

```
sharing_ratio = unique_blocks_served / total_block_requests
effective_capacity = physical_capacity / sharing_ratio
```

With N concurrent requests sharing a P-token system prompt:
```
saved_memory = (N - 1) × ceil(P / block_size) × block_bytes
```

### Fragmentation

```
internal_frag = (block_size - (seq_len mod block_size)) / block_size
wasted_bytes_per_seq = internal_frag × block_bytes  [for the last block only]
```

## 3. Required Observations

Before making block geometry and lifecycle decisions, the agent must measure:

| Observation | Why | How |
|-------------|-----|-----|
| Model KV dimensions | Determines per-token cost | Read model config: layers, kv_heads, head_dim, dtype |
| Sequence length distribution | Sets expected block count per request | Sample from request logs (p50, p95, p99) |
| Prefix commonality | Determines sharing potential | Radix tree hit rate at various block boundaries |
| Request arrival pattern | Determines concurrent sharing window | Measure cohort size over sliding windows |
| Token generation rate | Sets block sealing cadence | Tokens/second per request during decode |
| Model update frequency | Determines namespace churn rate | Deployment logs, version rotation schedule |
| Attention window size | Whether blocks expire positionally | Model architecture (full, sliding, sink+window) |
| Quantization mix | Whether multiple KV representations coexist | Serving config (FP16 prefill, INT8 decode, etc.) |

## 4. Alternatives (with prefer_when / avoid_when)

### 4.1 Block Size Selection

**Small blocks (1-4 tokens)**
- prefer_when: High prefix diversity, many short shared prefixes, fine-grained eviction needed, memory-constrained with high request heterogeneity
- avoid_when: Long sequences dominate, storage metadata overhead becomes significant (each block needs index entry), bulk sequential IO is the bottleneck

**Medium blocks (8-32 tokens)**
- prefer_when: Mixed workloads with moderate prefix sharing, good balance between sharing granularity and IO efficiency, disaggregated systems where block transfer is a unit of work
- avoid_when: Extreme prefix homogeneity (larger blocks would capture the shared portion more efficiently) or extremely short sequences where most blocks are partially filled

**Large blocks (64-256 tokens)**
- prefer_when: Long shared system prompts, homogeneous workloads, bulk transfer throughput critical, SSD-backed tiers where IO size matches device page alignment
- avoid_when: Diverse prefix lengths (wastes shared capacity on partially-overlapping blocks), high eviction pressure (evicting large blocks discards more useful KV), fine-grained sharing boundaries needed

**Evidence:** vLLM PagedAttention (SOSP 2023) uses 16-token blocks, achieving 2-4× throughput improvement via near-zero waste. SGLang RadixAttention found 6.4× speedup for multi-turn chat by enabling prefix sharing at token granularity using a radix tree, with blocks as the storage unit beneath.

### 4.2 Block Identity Scheme

**Content-addressed (hash of token IDs)**
- prefer_when: Prefix deduplication is primary goal, stateless lookup required, multiple producers may generate same KV independently
- avoid_when: Position-dependent attention (absolute positional embeddings without offset), blocks are rarely shared, hash computation cost matters at hot path

**Position-addressed (sequence_id + block_index)**
- prefer_when: No sharing expected, simple request-scoped lifetime, low metadata overhead critical
- avoid_when: Cross-request sharing desired, disaggregated serving where different nodes produce identical prefixes

**Radix-tree addressed (hierarchical prefix path)**
- prefer_when: Dynamic prefix sharing with variable-length common prefixes, multi-turn conversations where suffix diverges, need to discover sharing at lookup time without pre-registration
- avoid_when: Flat key-value store backing (radix requires tree traversal), extremely high insertion rate where tree maintenance is costly

**Evidence:** SGLang's RadixAttention uses a radix tree over token sequences, automatically discovering and sharing KV at any prefix boundary. Mooncake (FAST 2025) uses content-addressed hashing with a distributed hash ring, achieving 525% throughput improvement by enabling P2P sharing of KV blocks across a pool of prefill/decode nodes.

### 4.3 Sealing Policy

**Seal on block-full**
- prefer_when: Prefill phase (tokens arrive in bulk), maximizes block utilization, simplest correctness model
- avoid_when: Decode phase generates one token at a time (block remains unsealed for block_size decode steps)

**Seal on timeout (e.g., 100ms idle)**
- prefer_when: Decode phase where waiting for full block delays writeback unacceptably, preemption-prone environments where partial blocks must be persisted quickly
- avoid_when: High token rate where timeout rarely fires before block fills naturally, wasted space from partial blocks is expensive

**Seal on phase boundary (prefill→decode, or preemption)**
- prefer_when: Disaggregated prefill-decode architecture, need to transfer partial blocks between nodes immediately, preemption requires instant persistence
- avoid_when: Unified serving where prefill and decode share GPU memory and no transfer is needed

**Evidence:** DistServe (OSDI 2024) seals and transfers KV at the prefill-decode phase boundary, accepting partial last blocks to minimize TTFT. Mooncake similarly seals on chunk boundaries during chunked prefill, enabling immediate P2P transfer of completed chunks while prefill continues.

### 4.4 Namespace Design

**Model-version namespace (model_id + version hash)**
- prefer_when: Multiple model versions serve simultaneously (canary deployments, A/B tests), model weights change semantics of KV
- avoid_when: Single model, stable deployment with no version rotation

**Quantization-aware namespace (model + quant_scheme + calibration_id)**
- prefer_when: Mixed precision serving (FP16 prefill, INT8 decode), CacheGen-style compressed representations coexist with full-precision
- avoid_when: Uniform quantization across all paths, single representation stored

**Tenant-isolated namespace (tenant_id + model_id)**
- prefer_when: Multi-tenant serving where cross-tenant KV leakage is a security concern, compliance requires isolation
- avoid_when: Shared-nothing deployments, single-tenant systems, public models where sharing is desirable

**Evidence:** CacheGen (SIGCOMM 2024) stores multiple compressed representations (bitrates) of the same KV, requiring namespace separation by compression config. LMCache uses a composite key of (model_name, layer_idx, chunk_hash) enabling cross-session sharing within a model version.

### 4.5 Invalidation Strategy

**Cascade invalidation (invalidate prefix → invalidate all suffixes)**
- prefer_when: System prompt changes, model version rotation, any mutation to shared prefix content
- avoid_when: Suffix blocks have independent value (e.g., long decode sequences worth preserving even after prefix change)

**Sliding window expiry (positional invalidation)**
- prefer_when: Models with fixed attention window (Mistral-style sliding window), blocks beyond window boundary are provably useless
- avoid_when: Full attention models, models with attention sinks where initial tokens remain relevant

**TTL-based expiry (time-bounded validity)**
- prefer_when: RAG contexts with staleness bounds, real-time data in prompts, regulatory data retention limits
- avoid_when: Static system prompts, conversations where old context remains valid indefinitely

**Lazy invalidation (mark stale, GC later)**
- prefer_when: High write amplification from immediate deletion, batch GC is cheaper, stale blocks don't cause correctness errors (only waste space)
- avoid_when: Memory-constrained environments where stale blocks displace useful ones, strict consistency required between cache and model state

**Evidence:** SGLang's RadixAttention performs cascade invalidation through the radix tree—truncating a node invalidates all descendants. The prefix invalidation workload pattern shows that for a 64-block prefix with fan-out of 32 suffixes averaging 16 blocks each, a single prefix change triggers deletion of 576 blocks (64 + 32×16).

## 5. Coupled Constraints

| This Decision | Interacts With | Mechanism |
|---------------|---------------|-----------|
| Block size | Eviction granularity | Larger blocks = coarser eviction = more collateral damage to useful KV |
| Block size | IO transfer size | Must align with storage page size for efficiency (4KB NVMe pages, 2MB huge pages) |
| Block size | Prefix sharing hit rate | Smaller blocks = more prefix boundaries = higher sharing probability |
| Block size | Metadata overhead | Each block needs hash, ref count, timestamps: ~64-128 bytes. At 1-token blocks, metadata can exceed 0.1% overhead threshold |
| Sealing policy | Writeback latency | Aggressive sealing enables earlier offload but creates more partial blocks |
| Sealing policy | Disaggregation handoff | DistServe/Splitwise require sealed blocks before transfer; seal delay = TTFT penalty |
| Identity scheme | Distributed placement | Content-addressed hashing enables consistent-hash placement (Dynamo-style); position-addressed requires routing table |
| Namespace design | Cache effective capacity | Each namespace is a separate address space; blocks cannot be shared across namespaces even if content-identical |
| Invalidation scope | GC write amplification | Cascade invalidation of N suffix blocks on a prefix change requires N delete IOs |
| Block size | Compression ratio | CacheGen achieves 3.5-4.3× compression; larger blocks provide better compression context |

## 6. Failure Modes

### Geometry Failures

**Block too large + diverse prefixes**: Requests share first 100 tokens but blocks are 256 tokens. No sharing occurs because the shared prefix doesn't fill a complete block. The system stores N copies of what should be 1.
- Symptom: Low deduplication ratio despite high prefix commonality in token logs
- Detection: Compare `unique_tokens_stored / total_tokens_requested` against theoretical sharing ceiling

**Block too small + long sequences**: 1-token blocks for 128K context = 128K blocks per sequence. Metadata index grows to hundreds of MB. Lookup time dominates over transfer time.
- Symptom: Index operations (lookup, insert, delete) appear in profiles; per-block metadata exceeds 1% of block payload
- Detection: Measure `metadata_bytes / payload_bytes` ratio

### Sealing Failures

**Never seal during decode**: Decode generates 1 token every 20-50ms. With 16-token blocks, a block takes 320-800ms to fill. If preemption occurs mid-block, those tokens are lost (not persisted).
- Symptom: Preempted requests always cold-start on resume; no partial block recovery
- Detection: Track preemption-to-resume hit rate

**Seal too eagerly**: Every decode token seals its 1-token block immediately. Storage sees 50 tiny writes/second per request instead of 3 writes/second with 16-token blocks.
- Symptom: IOPS ceiling hit before bandwidth ceiling; write amplification from sub-page IOs
- Detection: Compare actual IOPS to `tokens_per_second / block_size` theoretical minimum

### Identity Failures

**Hash collision in namespace**: Two different token sequences hash to same block_id. One request reads another's KV. Attention computes over wrong values—output is silently corrupt.
- Symptom: Rare, non-reproducible output degradation
- Detection: Requires integrity check (store token_ids alongside KV, verify on load)

**Missing position in hash**: Content-addressed hash ignores position. With absolute positional embeddings, same tokens at different positions produce different KV. Sharing these blocks corrupts attention.
- Symptom: Degraded output quality when cache hit rate is high
- Detection: Perplexity regression test with cache enabled vs disabled

### Namespace Failures

**Stale namespace after model update**: Model weights change but namespace doesn't rotate. Old KV blocks serve new model—mathematically incompatible tensors flow into attention.
- Symptom: Immediate output quality collapse after deployment; cache hit rate is high but generation quality is low
- Detection: Canary evaluation after deployment; namespace version linked to model checkpoint hash

### Invalidation Failures

**Missing cascade**: Prefix block invalidated but suffix blocks remain addressable. A new request matches a suffix's parent path but retrieves orphaned suffix computed from old prefix KV.
- Symptom: Stale context leaking into responses; outputs reference deleted system prompt content
- Detection: Suffix blocks must verify parent chain integrity on load (hash chain or generation counter)

**Thundering herd on invalidation**: System prompt update invalidates prefix shared by 10K concurrent sessions. All sessions simultaneously cold-start, overwhelming prefill capacity.
- Symptom: Latency spike on system prompt rotation; prefill queue depth explodes
- Detection: Monitor queue depth during invalidation events; stagger rotation with grace periods

## 7. Hypotheses the Agent Can Generate

From the equations and constraints above, an agent can formulate:

1. **H: Reducing block size from 16 to 4 tokens will increase prefix sharing hit rate by >30% for this workload's prefix length distribution.**
   - Basis: If median shared prefix = 47 tokens, 16-token blocks share 2 full blocks (32 tokens), wasting 15 shareable tokens. 4-token blocks share 11 blocks (44 tokens).

2. **H: Sealing on timeout=50ms during decode will reduce preemption-resume cold starts by >50% with <5% write amplification increase.**
   - Basis: Median preemption arrives 200ms into decode. With 16-token blocks filling at 50 tok/s, blocks are 62% full at timeout vs 0% full without timeout sealing.

3. **H: Switching from position-addressed to content-addressed blocks will reduce storage footprint by >40% given the measured 60% system-prompt overlap.**
   - Basis: 60% of stored blocks are duplicates of shared prefix. Content-addressing deduplicates at write time.

4. **H: The current block size creates internal fragmentation >15% given the observed sequence length distribution.**
   - Basis: If seq_len mod block_size yields avg 8 wasted tokens per sequence at block_size=16, and average seq has 20 blocks, fragmentation = 8/(20×16) = 2.5%. But if many short sequences (avg 48 tokens = 3 blocks), last-block fragmentation = 8/48 = 16.7%.

5. **H: Adding quantization-aware namespaces will enable 2× effective cache capacity by storing INT8 KV for decode without evicting FP16 KV needed for prefill accuracy.**
   - Basis: INT8 is half the bytes of FP16. If decode tolerates INT8 (shown by CacheGen: <1% quality loss), a dual-representation scheme doubles effective token capacity.

6. **H: Cascade invalidation of the primary system prompt will trigger >10K block deletions and a 500ms GC pause given current suffix fan-out.**
   - Basis: Observed fan-out of 32 suffixes × avg 16 blocks/suffix + 64 prefix blocks = 576 blocks. At 10K active sessions, total = 5.76M block deletes if no deduplication; with deduplication = 576 unique deletes but ref-count decrements for 10K sessions.

## 8. Experiments and Falsifiers

### E1: Block Size Sensitivity Sweep

**Tests H1, H4.** Vary block_size ∈ {1, 4, 8, 16, 32, 64, 128} tokens.
- Measure: dedup ratio, metadata overhead ratio, IO throughput (MB/s), p99 lookup latency
- Falsifier for H1: If dedup ratio improvement < 10% when moving from 16→4, prefix distribution doesn't align with block boundaries as hypothesized
- Control: Same workload trace, same total tokens, vary only block size

### E2: Seal Policy Comparison

**Tests H2.** Three policies: {block-full, timeout-50ms, timeout-200ms} under decode with injected preemptions.
- Measure: preemption recovery hit rate, write IOPS, write amplification, partial-block waste ratio
- Falsifier for H2: If preemption recovery < 20% improvement OR write amplification > 20% increase, the timeout doesn't improve the system enough to justify complexity
- Control: Same request sequence, same preemption schedule

### E3: Content-Addressed vs Position-Addressed

**Tests H3.** Deploy both schemes on same request trace.
- Measure: unique blocks stored, total bytes written, lookup latency, sharing ratio
- Falsifier for H3: If unique_blocks(content) / unique_blocks(position) > 0.7, the workload doesn't have enough prefix sharing to justify content addressing overhead
- Control: Identical requests replayed, only addressing scheme changes

### E4: Namespace Collision Stress Test

**Tests correctness.** Generate requests targeting namespace boundary conditions.
- Measure: Output quality (perplexity) with cache enabled vs disabled, across model version transitions
- Falsifier: If perplexity delta > 0.1 with cache enabled after model rotation, namespace isolation has failed
- Method: Deploy model v2, keep v1 blocks in cache, measure whether v1 blocks are ever served to v2 requests

### E5: Invalidation Cascade Cost

**Tests H6.** Trigger system-prompt invalidation under varying suffix fan-out.
- Measure: GC duration, delete IOPS burst, p99 latency of concurrent requests during GC
- Falsifier for H6: If GC completes in <50ms even at 10K fan-out, cascade cost is negligible and lazy invalidation complexity isn't justified
- Control: Vary fan-out ∈ {10, 100, 1K, 10K} with fixed prefix length

### E6: Fragmentation Under Real Distributions

**Tests H4.** Replay production sequence-length distribution against various block sizes.
- Measure: wasted bytes (partial last blocks), total blocks created, effective utilization
- Falsifier for H4: If internal fragmentation < 5% at current block size, geometry change isn't worthwhile
- Method: Histogram seq_len mod block_size from production traces

## 9. Production Evidence

### vLLM PagedAttention
- **System:** vLLM (Kwon et al., SOSP 2023)
- **Problem:** GPU memory waste from pre-allocated contiguous KV buffers; 60-80% of allocated KV memory wasted due to fragmentation and over-reservation
- **Mechanism:** Paged virtual memory applied to KV: fixed-size blocks (16 tokens), non-contiguous physical allocation, block table for indirection
- **Result:** 2-4× throughput improvement by fitting 2-4× more concurrent sequences in the same GPU memory
- **Lesson:** Block size of 16 tokens is a sweet spot—small enough for low fragmentation (<6.25% worst case for last block), large enough for efficient memory management

### SGLang RadixAttention
- **System:** SGLang (Zheng et al., 2024)
- **Problem:** Multi-turn and multi-call LLM programs repeat large prefixes across calls; each call recomputes shared prefix KV
- **Mechanism:** Radix tree indexes KV blocks by token content, enabling automatic prefix sharing without user annotation; LRU eviction at block granularity
- **Result:** 6.4× speedup on multi-call workloads (tree-of-thought), 5× on multi-turn chat by eliminating redundant prefill computation
- **Lesson:** Content-based addressing with hierarchical structure (radix tree) enables discovery of sharing at arbitrary prefix boundaries, not just pre-defined system prompt lengths

### Mooncake KV Cache Transfer
- **System:** Mooncake (Qin et al., FAST 2025), Kimi/Moonshot production
- **Problem:** Disaggregated prefill-decode requires fast KV transfer; centralized storage becomes bottleneck at scale
- **Mechanism:** Distributed KV cache pool using consistent hashing for placement, P2P transfer between prefill and decode nodes, chunked transfer that pipelines with computation
- **Result:** 525% throughput improvement over baseline; reduced KV transfer latency to overlap with prefill computation on subsequent chunks
- **Lesson:** Block identity based on content hash enables placement via consistent hashing (Dynamo-style); chunked/pipelined transfer amortizes sealing latency across prefill chunks rather than waiting for full sequence completion

### CacheGen Compressed KV
- **System:** CacheGen (Liu et al., SIGCOMM 2024)
- **Problem:** KV cache transfer between nodes bottlenecked by network bandwidth; raw KV for 4096 tokens on large models exceeds 1 GB
- **Mechanism:** Custom codec compresses KV tensors using learned delta coding and adaptive quantization; stores compressed representations alongside metadata for decompression
- **Result:** 3.5-4.3× compression ratio with <1% quality degradation (perplexity increase <0.2); reduced network transfer time proportionally
- **Lesson:** Block identity must accommodate multiple representations (compressed/uncompressed) of same logical KV; namespace must encode compression scheme and parameters to prevent serving incompatible formats

### FlexGen SSD Offload
- **System:** FlexGen (Sheng et al., ICML 2023)
- **Problem:** GPU memory insufficient for large-batch inference; need to offload KV to CPU memory and SSD
- **Mechanism:** Linear offload policy computes optimal fraction of KV per layer on each tier (GPU/CPU/SSD); block size aligned to SSD page size for IO efficiency
- **Result:** Enabled 100× larger batch sizes on single GPU; throughput-optimal when SSD IO aligned to 512KB-2MB transfer units
- **Lesson:** Block geometry must consider storage device characteristics. Sub-page blocks waste IO bandwidth; blocks larger than SSD internal page (typically 4-16KB for NAND, but effective IO unit is much larger due to controller striping) are required for sequential throughput. FlexGen's 4MB effective block aligns with SSD controller stripe width.

### InfiniGen Selective Retrieval
- **System:** InfiniGen (Lee et al., OSDI 2024)
- **Problem:** Offloaded KV cache contains blocks that aren't needed for current attention step; loading all blocks wastes bandwidth
- **Mechanism:** Predicts which KV blocks will have high attention scores using lightweight prefetcher; only retrieves predicted-essential blocks from offload tier
- **Result:** Achieved near-full-attention quality while loading only 10-30% of offloaded KV blocks per decode step
- **Lesson:** Not all blocks in a sequence have equal lifetime value. Block identity should support selective retrieval (per-layer, per-head block addressing) rather than forcing all-or-nothing sequence load. This implies sub-sequence addressability in the block identity scheme.

### DistServe / Splitwise Disaggregation
- **System:** DistServe (Zhong et al., OSDI 2024) and Splitwise (Patel et al., ISCA 2024)
- **Problem:** Prefill and decode have conflicting resource requirements; co-locating them on same GPU causes mutual interference
- **Mechanism:** Separate prefill and decode onto different GPU pools; transfer sealed KV blocks between them via network or shared storage
- **Result:** DistServe achieved 2-10× TTFT reduction by isolating prefill from decode batch pressure. KV transfer time becomes critical path for time-to-first-token after prefill completes.
- **Lesson:** Sealing at phase boundary (prefill→decode) is non-negotiable in disaggregated architectures. Partial last blocks must be transferred immediately—waiting for a full block adds block_size/token_rate latency to TTFT. The transfer unit IS the sealed block, making block size directly determine minimum transfer granularity and network utilization efficiency.

## Implications for KV Block Storage

1. **Block size is the fundamental tuning knob** bridging compute semantics and storage efficiency. The 16-token default from vLLM is reasonable for GPU memory management but may not be optimal for NVMe tiers (where 256KB-4MB IO alignment matters more than token-level fragmentation).

2. **Content-addressed identity is prerequisite for efficient shared storage.** Any system aspiring to prefix deduplication across requests or nodes must hash blocks by content. Position-addressing makes sharing impossible without a separate lookup layer.

3. **Sealing defines the boundary between ephemeral and persistent.** The storage system should only ever see sealed blocks. Sealing policy is owned by the serving engine, but the storage system's contract must define: minimum block fill ratio accepted, whether partial blocks are padded or variable-length, and maximum seal-to-visible latency.

4. **Namespace is the correctness firewall.** A block served from wrong namespace (wrong model version, wrong quantization) produces silently corrupt output. Storage must enforce namespace isolation with the same rigor as filesystem permissions—not as a convenience but as a correctness invariant.

5. **Invalidation is the storage system's most expensive operation at scale.** A single system prompt rotation can cascade into millions of ref-count decrements or block deletions. Storage design must account for this: batch-friendly delete paths, background GC, and grace periods that prevent thundering-herd refill.

6. **Multi-representation storage (full-precision + compressed + quantized) is emerging as standard.** The namespace scheme must accommodate multiple representations of logically identical KV without conflating them. CacheGen's 3.5× compression means network-tier and SSD-tier might store different representations of same logical block.

7. **Sub-block addressability is needed for selective retrieval.** InfiniGen's result (only 10-30% of blocks needed per step) means the storage system benefits from per-layer or per-head-group addressing within a token-block, enabling partial loads without reading entire multi-layer blocks.
```

Here's the complete domain reference file for `kv-footprint-and-lifecycle`. It covers all 9 required sections across ~400 lines, with:

- Governing equations (per-token KV size, block size, fragmentation) with worked examples for real models
- 5 decision areas (geometry, identity, sealing, namespace, invalidation) each with specific prefer_when/avoid_when conditions
- Production evidence from 7 systems with specific numbers (vLLM 2-4×, SGLang 6.4×, Mooncake 525%, CacheGen 3.5-4.3×, etc.)
- 6 testable hypotheses with quantified predictions
- Matched experiments with explicit falsification criteria
- Coupled constraints table showing cross-decision interactions
- "Implications for KV Block Storage" section linking all findings to storage system design decisions

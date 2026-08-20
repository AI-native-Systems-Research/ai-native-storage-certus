# Inference Workload → Storage IO Pattern Mapping

Mapping high-level LLM inference serving behaviors to concrete storage access patterns for synthetic workload generation in certus-api-bench.

**Research scope**: 8 systems/papers investigated (FlexGen, InstInfer, InfiniGen, Mooncake, DistServe/Splitwise, Sarathi-Serve, BurstGPT, SpecInfer) plus vLLM/SGLang/llm-d source code analysis. No existing storage-level KV cache workload generator found in literature.

### Quick Reference: Store vs Load per Pattern

**Important**: Loads from certus only happen when blocks were previously **Stored AND then evicted** from GPU/DRAM. If blocks are still in GPU cache (prefix hit + not evicted), there is ZERO certus IO. The patterns below assume the system is in the **spilling regime** (working set > GPU+DRAM capacity), which is when certus matters.

| Pattern | Store (GPU→DRAM→SSD) | Load (SSD→DRAM→GPU) | Dominant (in spilling regime) |
|---------|---------------------|--------------------:|----------|
| §1 Prefill | New unique tokens only | Prefix blocks that were evicted | **Load** if prefix was evicted; else pure **Store** |
| §2 Cohort sharing | First request stores prefix | N-1 requests Load if prefix evicted | **Load** (only when eviction happened between requests) |
| §3 Decode | 1 block per 16 tokens (background) | Full block on cache miss (critical path) | **Load** under pressure, else **0 IO** |
| §4 Eviction | No IO (DRAM pointer free; SSD write already done by background writer) | Cold Load on reaccess (SSD→DRAM→GPU) | **Cold Load** on reaccess; eviction itself is free |
| §5 Shared→Unique | First stores prefix + all store M unique | N-1 Load K shared (if evicted) | **Store** always + **Load** if evicted |
| §6a Chunked prefill | Chunks of new tokens stored | Cached chunks loaded (if evicted) | Mixed per chunk |
| §6b Speculative | 3–5× Store amplification (most discarded) | Load shared prefix for verification | **Store** (amplified) |
| §6c Beam search | COW Store only at divergence | B × Load of shared blocks per step | **Load** dominated |
| §6d Continuous batch | Prefill stores + eviction stores | Decode loads + reschedule loads | **Both** async |
| §6e Multi-turn | Trickle stores (new generation) | Prior turns' KV (if evicted between turns) | **Load** if evicted; else 0 |
| §6f Disaggregated | Bulk Store (prefill node, GB-scale) | Bulk Load (decode node, GB-scale) | **Both** (always — different nodes) |
| §6g Sparse retrieval | Standard (1/16 tokens) | Sparse subset Load (10–30% of blocks) | **Load** (reduced) |
| §6h LRU prefix-aware | Eviction stores hit unique-suffix | Reload loads hit unique-suffix | **Both** (biased to suffix) |

**Key principle**: Store is policy-dependent (not unconditional). The connector's `prepare_store` filters by: dedup (Check — skip if block already exists), admission policy (prompt-only, reuse-threshold), and capacity (Reserve may reject if DRAM tier is saturated). Load happens ONLY when a block that was Stored is not currently on GPU and is needed again. Which tier serves the Load determines latency.

### Three-Tier Data Path (Certus Architecture)

```
GPU HBM  ←──────→  DRAM (memory-tier, 32 GiB)  ←──────→  SSD (NVMe × 4)
         cudaMemcpy                              SPDK NVMe R/W
         D2H: ~12 GB/s                           read: ~14 GB/s (4 drives)
         H2D: ~12 GB/s                           write: ~10 GB/s (4 drives)
```

**Store (Populate)** is always two hops:
1. GPU → DRAM: `cudaMemcpy D2H` into memory-tier slot (immediate, on critical path of populate)
2. DRAM → SSD: Background writer flushes asynchronously (NOT on critical path)

**Load (Lookup)** depends on which tier holds the block:

| Path | Hops | Latency | When |
|------|------|---------|------|
| **Hot Load** | DRAM → GPU | ~330 µs per 4 MB block | Block still in memory-tier (not evicted from DRAM) |
| **Cold Load** | SSD → DRAM → GPU | ~1–3 ms per 4 MB block | Block was evicted from DRAM, must read from NVMe first |

**Eviction** moves blocks between DRAM tiers:
- Memory-tier full → LRU block's DRAM slot freed (block still on SSD from background write)
- This is NOT an IO operation — just a pointer removal from the dispatch map
- The block remains on SSD; a future Load becomes a Cold Load instead of Hot Load

**Why this matters for bench design**:
- certus-api-bench v2 `hot lookup` = **Hot Load** (DRAM → GPU only, measures cudaMemcpy throughput)
- certus-api-bench v2 `cold lookup` = **Cold Load** (SSD → DRAM → GPU, measures NVMe + DMA pipeline)
- Real inference mixes both: recently-stored blocks are Hot (still in DRAM), older evicted blocks are Cold
- The DRAM tier size (32 GiB) determines what fraction of Loads are Hot vs Cold

---

## 1. KV Cache Prefill → Load (prefix hit) + Store (new tokens)

**Inference behavior**: A new request arrives. With prefix caching enabled (RadixAttention), the scheduler checks how much of the prompt's KV already exists in the offload tier.

**Two sub-cases:**

### 1a. No prefix hit (cold prefill) — Pure Store
- Model computes KV for ALL prompt tokens from scratch
- All blocks are new → sequential Store to certus
- This is the simple case (first request with unique content)

### 1b. Prefix cache hit (warm prefill) — Load THEN Store
- Scheduler finds K prefix blocks already offloaded (from a prior request with same prefix)
- **LOAD**: K blocks loaded from SSD/DRAM back to GPU (the shared prefix)
- Model computes KV ONLY for the M tokens after the prefix hit point
- **STORE**: M new blocks written to certus (the unique suffix)
- Net IO: K reads + M writes (where K + M = total prompt blocks)

**Storage pattern** (combined):
- Load phase: K blocks read (prefix), sequential from block 0..K
- Store phase: M blocks written (suffix), sequential from block K..K+M
- With high prefix hit ratios (>60% in production), LOADS DOMINATE prefill IO
- Concurrency: Multiple requests prefilling simultaneously (continuous batching)
- Semantics: Write-once, content-addressed (dedup by hash — same prefix = same blocks, skip store)

**Bench parameters**:
| Parameter | Typical range | Notes |
|-----------|--------------|-------|
| Block size | 2 MB (Llama-8B) – 80 MB (Llama-70B, factor=16) | Cross-layer layout |
| Total blocks per prefill | 64–512 | 1K–8K prompt tokens / 16 tokens per block |
| Prefix hit ratio | 0% (cold) – 90% (warm, shared system prompt) | Determines Load vs Store mix |
| Load blocks (K) | 0–460 | hit_ratio × total_blocks |
| Store blocks (M) | 50–512 | (1 - hit_ratio) × total_blocks |
| Concurrent prefills | 1–8 | Limited by GPU memory |

---

## 2. Cohort Sharing → N × Load of Same Blocks

**Inference behavior**: Multiple requests share a common system prompt or document prefix. The KV cache for this shared prefix is computed once (by the first request) and reused by subsequent requests via prefix caching (RadixAttention).

**Storage operations**:
- First request in cohort: **Stores** the shared prefix blocks (cold prefill, §1a)
- All subsequent requests: **Load** the same K blocks from storage (prefix cache hit)
- Net: 1 Store + (N-1) Loads of the same K blocks — **Load-dominated**
- Temporal: Loads may be simultaneous (batch arrival) or staggered
- Sharing ratio: 60–90% of tokens shared (system prompt + few-shot examples)

**Bench parameters**:
| Parameter | Typical range | Notes |
|-----------|--------------|-------|
| Cohort size | 2–64 sessions | Same system prompt or document |
| Shared blocks | 32–256 blocks (512–4096 tokens shared) | System prompts are often 1K–4K tokens |
| Read concurrency | Up to cohort_size × blocks_per_read | All cohort members reading prefix simultaneously |
| Divergence point | After shared_blocks, each session unique | Fan-out pattern |

**Key insight**: The shared prefix blocks are hot — they're read N times but written once. This is the primary caching opportunity and the pattern that stresses read path most.

---

## 3. Decode Phase → Store (new block) + Load (only on preemption reschedule)

**Inference behavior**: After prefill, the model generates tokens one at a time. Attention reads ALL previous KV — but this happens **entirely on GPU** (attention kernel reads GPU HBM directly). No certus IO is needed for the attention read itself.

**When does certus IO actually happen during decode?**

| Operation | Trigger | Why | Latency |
|-----------|---------|-----|---------|
| **Store** (new block full) | Every 16 generated tokens complete a block | Offloading manager calls `prepare_store` for durability/sharing | Background (deferred, not on critical path) |
| **Load** (preemption reschedule) | Scheduler preempted this request, freed its GPU blocks, then rescheduled it | Request's KV was evicted → must be reloaded before decode can resume | **Critical path** — decode BLOCKED until Load completes |

**The decode attention itself does NOT read from certus.** It reads from GPU block memory. A Load only happens because:
1. Scheduler **preempts** the request (frees GPU blocks to make room for another request)
2. Freed blocks get Stored to certus (eviction)
3. Later, scheduler **reschedules** the request
4. Its KV blocks must be **Loaded** back from certus → GPU before decode can resume

**Storage pattern**:
- **Stores**: Fixed rate — 1 Store per 16 new tokens, unconditional. Sequential per request.
- **Loads**: Only on preemption reschedule. NOT every decode step. Frequency depends on how often the scheduler preempts and reschedules (driven by MAX_NUM_SEQS vs active requests).
- **Zero IO case**: If request is never preempted, decode does Stores only (background). No Loads.

**Bench parameters**:
| Parameter | Typical range | Notes |
|-----------|--------------|-------|
| Store frequency | 1 block per request per 16 tokens | Fixed, unconditional |
| Load trigger | Preemption + reschedule | NOT every decode step |
| Load size | All of request's KV blocks | Full reload of evicted context |
| Preemption rate | 0 (enough GPU slots) – frequent (MAX_NUM_SEQS << active requests) | Depends on over-subscription |
| Load latency | Critical path | Decode blocked until complete |

**Key insight**: Decode's relationship to certus is asymmetric. Stores happen unconditionally (trickle, background). Loads happen only on preemption — but when they do, they're on the **critical path** for TPOT because decode cannot resume until the request's full KV is back on GPU.

---

## 4. Eviction Pressure → DRAM Free (no IO) + Cold Load (on reaccess)

**Inference behavior**: DRAM memory-tier is full. New Stores need slots. LRU eviction frees cold DRAM slots to make room. When an evicted block is needed again later, it must be Cold-Loaded from SSD.

**Critical clarification**: Eviction from DRAM is **NOT an IO operation**. The data is already on SSD (written by the background writer during the original Store). Eviction just:
1. Transitions dispatch-map entry from `MemoryTier{pointer}` → `BlockDevice{offset}`
2. Frees the DRAM slot

The block remains on SSD — a future access becomes a Cold Load (SSD→DRAM→GPU) instead of a Hot Load (DRAM→GPU).

**Storage operations:**

| Operation | When | What happens | IO? |
|-----------|------|-------------|-----|
| **Eviction** (DRAM free) | Memory-tier full, new Store needs slot | Pointer transition + DRAM free | **NO IO** — data already on SSD |
| **Cold Load** (on reaccess) | Evicted block needed by prefill/decode | SSD read → DRAM → GPU copy | **Yes** — NVMe read + H2D DMA |

**The key contention**: New Stores (which trigger eviction to free DRAM) happen **simultaneously** with Cold Loads (which need DRAM slots + NVMe bandwidth). Both compete for:
- DRAM slots (Store needs a slot, Load promotes into a slot)
- NVMe bandwidth (background writer flushing vs Cold Load reading)
- This is the bidirectional contention v3 measures.

**Storage pattern**:
- Eviction itself: Zero IO. Just pointer state change.
- The REAL IO is: background writer Stores (DRAM→SSD) from earlier + Cold Loads (SSD→DRAM→GPU) from reaccess.
- Contention: Store background writes and Cold Load reads on same NVMe drives simultaneously.

**Bench parameters**:
| Parameter | Typical range | Notes |
|-----------|--------------|-------|
| Eviction batch size | 1–32 blocks per cycle | Watermark-based triggering |
| Store pattern | Random (scattered, LRU) | Unlike sequential prefill stores |
| Load pattern | Random (reaccessed blocks) | Triggered by scheduler reschedule |
| Contention | Simultaneous stores + loads | THE key stress scenario |
| Capacity ratio | working_set / cache_capacity = 1.2–5× | Over-subscription ratio |
| Eviction frequency | Per scheduling step if under pressure | Could be every 1–5ms |

---

## 5. Shared Prefix → Unique Suffix (The Compound Pattern)

**Inference behavior**: A cohort of N sessions shares a common tool output, document, or system prompt, then each session continues with its own unique conversation. This is the canonical pattern for:
- Multi-user chat with same system prompt
- RAG where multiple queries hit the same retrieved document
- Tool-use where multiple agents share the same tool output
- Batch processing of similar requests

**Storage operations:**

```
Time →

Session 1: [LOAD shared blocks 0..K] → [STORE unique blocks K+1..K+M₁]
Session 2: [LOAD shared blocks 0..K] → [STORE unique blocks K+1..K+M₂]
Session 3: [LOAD shared blocks 0..K] → [STORE unique blocks K+1..K+M₃]
...
Session N: [LOAD shared blocks 0..K] → [STORE unique blocks K+1..K+M_N]
```

- Phase 1 (shared): **N × Loads** — same K blocks loaded N times (high read amplification)
- Phase 2 (unique): **N × Stores** — each session writes its own unique blocks (parallel sequential stores)
- First session in cohort: no Load (it computes and Stores the prefix). Subsequent sessions Load it.
- Transition: Sharp boundary at divergence point

**Note**: The first request that establishes the shared prefix does a pure Store (§1a cold prefill). All subsequent cohort members do Load (shared) + Store (unique). So the overall pattern is: 1 Store of K blocks + (N-1) × Load of K blocks + N × Store of M blocks.

**Bench parameters**:
| Parameter | Typical range | Notes |
|-----------|--------------|-------|
| N (cohort size) | 4–64 | Sessions sharing prefix |
| K (shared blocks) | 32–256 | Shared prefix length |
| M (unique blocks per session) | 16–512 | Varies by generation length |
| Arrival stagger | 0–5s between sessions | Simultaneous vs staggered |
| Load amplification | (N-1) × K | Total shared loads |
| Store parallelism | N independent streams | After divergence |

---

## 6. Additional Patterns (Validated via Research)

### 6a. Chunked Prefill (Sarathi-Serve)
- Long prompts split into fixed chunks (512–2048 tokens), processed across multiple scheduler steps
- **Per chunk**: may Load prefix-cached blocks (if chunk overlaps with cached prefix) + Stores newly computed blocks
- Without prefix hit: each chunk = pure **Store** of ~32 blocks, then yields to decode steps
- With prefix hit: first chunks may be pure **Load** (prefix), later chunks = **Store** (new)
- Temporal: interleaved Load/Store bursts within a single request lifecycle
- More gradual IO pressure than monolithic prefill

### 6b. Speculative Decoding / Tree Verification (SpecInfer)
- Multiple candidate branches generated speculatively (branching factor 3–5)
- **Normally NO extra certus IO**: speculative KV stays on GPU during verification. Rejected branches are freed from GPU without ever being Stored (they never became committed full blocks)
- **Certus IO only if**: the connector eagerly offloads uncommitted branches (not default behavior) OR GPU pressure evicts speculative blocks before verification
- **If offloaded**: 3–5× Store amplification (most discarded immediately after verification rejects them)
- Pattern in practice: **Mostly GPU-local; certus sees only committed blocks post-verification**

### 6c. Beam Search / Parallel Sampling
- B beams share prefix KV, then diverge at each step
- **Normally GPU-local**: beams share GPU block-table references to prefix blocks (no certus Load per beam)
- **Certus IO only if**: prefix was evicted from GPU (then Cold Load to restore it) OR new beam-divergence blocks are offloaded
- **Stores**: New suffix blocks at divergence points (committed blocks, not COW at storage level)
- Pattern in practice: **Suffix Stores only; shared prefix stays GPU-resident unless evicted**
- NOT "B × Load per step" — that would be the GPU attention access pattern, not certus IO

### 6d. Continuous Batching Steady-State
- No global prefill/decode phases — individual requests in different phases simultaneously
- Simultaneous: some requests doing prefill **Stores** + some doing reschedule **Loads** (preempted requests restored)
- KV blocks allocated/freed **asynchronously** (not synchronized across requests)
- Arrival bursts (per BurstGPT traces: 10–100× variation) → memory pressure varies → preemption rate varies
- Pattern: **Mixed async Store+Load, no synchronized phases** — this IS the steady-state the server sees
- **Priority preemption cascades**: high-priority requests displace many low-priority → correlated burst of Loads when low-priority reschedules (thrashing pattern)

### 6e. Multi-Turn Conversation Reuse
- KV cache from prior turns persisted to storage
- **Load** (turn start): Only the **absent** prior-turn blocks are Loaded (blocks still in GPU/DRAM are hit locally — not "ALL prior turns")
- **Store** (during generation): Only NEW blocks from this turn stored (prefix-hit blocks are NOT re-stored, dedup via Check)
- Growing Load set: depends on what was evicted between turns. If all prior turns evicted → massive Load. If still resident → 0 Load.
- Temporal: **Load burst at turn-start** (size depends on eviction between turns), then trickle of Stores
- This is BENCH_TARGET's primary pattern
- In BENCH_TARGET (256 convs, 64 MAX_NUM_SEQS, 32 GiB tier): by late turns, most prior-turn blocks HAVE been evicted → Loads are large

### 6f. Disaggregated Prefill-Decode Architecture (DistServe, Splitwise, Mooncake)
- Prefill node computes KV for entire prompt → **Stores** all blocks to shared storage
- Decode node receives the request → **Loads** ALL those blocks from storage before it can start generating
- Storage = communication channel between nodes
- Pattern: **Bulk Store (prefill node) → Bulk Load (decode node)** — one-shot, GB-scale
- Size: Llama-70B at 4K tokens ≈ 2.5–5 GB per request
- Latency-critical: decode node **blocks** until all Loads complete (cannot generate until full KV is on GPU)
- Bandwidth-sensitive: end-to-end latency = Store time + propagation + Load time

### 6g. Selective/Sparse KV Retrieval (InfiniGen, Attention Sink)
- Not all prior KV entries equally important — systems retrieve only predicted-important subset
- InfiniGen (OSDI 2024): selects important **tokens/layers**, not blocks
- **Out of scope for certus as-is**: Certus transfers fixed cross-layer blocks (2 MB). Cannot Load 10% of a block — it's all-or-nothing per block. Sparse retrieval would require a sub-block selection/packing mechanism not currently implemented.
- **What IS representable**: If important tokens cluster into a subset of blocks, then fewer blocks need Loading. But this depends on token→block mapping (clustering), and may be zero savings if important tokens are scattered across all blocks.
- **Attention sinks** (different from InfiniGen): fixed "hot head" blocks (first few tokens) + sliding tail. Creates bimodal access: head blocks are always Hot (never evicted), tail blocks cycle through DRAM. Not sparse retrieval — it's just LRU naturally keeping head blocks hot.

### 6h. LRU Eviction with Prefix Awareness (vLLM)
- vLLM evicts from free-queue head (LRU)
- Freed blocks added in **reverse order**: last block (most unique) evicted first
- **DRAM eviction** (no IO): unique-suffix blocks lose their DRAM slot first, shared-prefix blocks stay longer
- **Cold Loads** (reload): when evicted unique-suffix blocks are needed again → Cold Load from SSD
- Net effect: Cold Loads disproportionately hit **unique-suffix blocks**, shared-prefix blocks rarely need Cold Loads
- Implication for bench: Cold Load IO is concentrated on the "tail" of each session, not the shared head
- Per Xinnor eviction study (30 traces, 8M IO events): LRU is near-optimal for hit ratio; TinyLFU+SLRU has 10× lower write amplification but 33 percentage points worse hit ratio

### 6i. Prefix Invalidation / Hash Chain Break
- Changed system prompt, tool schema, model adapter, or tokenizer → prefix block hashes change
- All hash-chained suffix blocks also invalidated (Merkle chain broken)
- **Result**: Previously-stored prefix becomes orphaned on SSD (dead data, needs GC). New Stores for the changed prefix + all suffix blocks (full recompute)
- Pattern: **Burst of new Stores (entire invalidated chain) + lost Load hits + SSD garbage accumulation**
- Frequency: Every system prompt update, adapter swap, or tool schema change

### 6j. Context Overflow / Left Truncation
- Request exceeds MAX_MODEL_LEN → leftmost tokens truncated
- Truncation creates new block hashes (different token content at position 0) → existing stored blocks no longer match
- Pattern: **Store churn** — new blocks stored for the shifted content, old blocks become garbage
- Similar to §6i but triggered by request length rather than config change

### 6k. Sliding Window Attention
- Model uses fixed attention window (e.g., 4096 tokens) — only last W tokens' KV needed for attention
- Blocks older than window expire sequentially as new tokens generate
- Pattern: **Sequential block expiry** — oldest blocks become dead at fixed rate. DRAM slots freed in order.
- On certus: expired blocks are never Cold-Loaded again (they'll never be needed). Reduces useful working set.
- Creates moving-tail residency: only the W most recent tokens' blocks are "live"

### 6l. SSD Capacity Eviction / GC
- Unlike DRAM eviction (pointer free, data stays on SSD), **SSD full** = true data loss
- If SSD is full and a new block needs to be written, certus must either: reject the Store, or delete old SSD extents
- After SSD deletion: future Load for that block = **miss** → recomputation required (no data anywhere)
- Pattern: **True miss → recompute → Store** (unlike DRAM eviction which is just Cold Load)
- Not modeled in current bench (SSD is assumed infinite relative to working set)

---

## 7. Realistic Parameter Summary

From kv_IO_pattern.md, vLLM source analysis, and Xinnor MLPerf Storage 3.0 measurements:

| Dimension | Value | Source |
|-----------|-------|--------|
| Block size (cross-layer) | 2–5 MB (TP=1), 0.25–0.6 MB (TP=8) | kv_IO_pattern.md formula |
| Offloaded block (factor=16) | 32–80 MB | 16 GPU blocks bundled |
| Offloaded block (factor=1) | 2–5 MB | 1 GPU block |
| Tokens per block | 16 (default) | vLLM block_size config |
| Concurrent readers (llm-d) | 48 threads (64 × 0.75) | kv_IO_pattern.md |
| Prefix hit ratio (production) | 60–90% | RadixAttention / SGLang papers |
| Typical prompt length | 1K–8K tokens | Depends on use case |
| Typical generation length | 256–4K tokens | Depends on use case |
| Scheduling step interval | 1–10 ms | Continuous batching cadence |
| Active requests (continuous batch) | 32–256 | GPU memory dependent |
| Agent context growth | 10K–200K+ tokens | Xinnor blog (complex agents) |

### Measured IO Characteristics (Xinnor MLPerf Storage 3.0)

| Dimension | Prefill Phase | Decode Phase | Source |
|-----------|--------------|-------------|--------|
| IO size (avg) | 140–328 MB | 11–18 MB | Xinnor traces |
| IO size (peak) | ~1 GB | 23 MB | Xinnor traces |
| Bandwidth (sustained) | 8.7–9.5 GB/s write | 13–27 GB/s read | Xinnor MLPerf |
| Bandwidth (peak) | 25 GB/s write | up to 200 GiB/s (model-dependent) | Xinnor blog |
| Queue depth (peak) | 44–75 | 44–75 | Xinnor traces |
| Read/write ratio | Near-zero reads | Strongly read-dominant | Xinnor blog |

### Eviction Policy Findings (Xinnor, 30 traces, 8M IO events, 281 GB working set)

| Policy | Hit Ratio | Write Amplification | Notes |
|--------|-----------|--------------------:|-------|
| ARC, LRU, 2Q | Highest (within ~1 pp) | Higher (~0.35) | LRU wins by simplicity |
| TinyLFU+SLRU | 33 pp below top | ~0.035 (10× lower) | Filters single-use data |

LRU is near-optimal for hit ratio. TinyLFU trades hit ratio for much lower write amplification (relevant for SSD endurance).

---

## 8. Mapping to certus-api-bench Workload Modes

Proposed bench scenarios (v4):

| Scenario | Primary Pattern | Key Stress Point |
|----------|----------------|-----------------|
| `prefill-burst` | §1 | Sequential write throughput, concurrent streams |
| `prefix-sharing` | §2 + §5 | Read amplification, cache hit rate |
| `decode-steady` | §3 | Mixed random read + small append, latency |
| `eviction-thrash` | §4 | Read/write contention, LRU churn |
| `cohort-diverge` | §5 | Shared→unique transition, fan-out |
| `disaggregated-pipeline` | §6f | Producer-consumer over storage, tail latency |
| `multi-turn-grow` | §6e | Growing read set, conversation lifecycle |

---

## 9. Research Findings & Resolved Questions

### Answered

- **No existing KV cache storage workload generator exists.** BurstGPT has request-level traces (arrivals, prompt/response lengths) but no block IO. YCSB/FIO have no inference semantics. **certus-api-bench-v4 would be first.**
- **FlexGen vs vLLM IO pattern**: FlexGen uses LP-scheduled "zig-zag" transfers — entire layer KV tensors in bulk, batch-oriented. Targets throughput (offline), not latency. Much simpler than vLLM's per-block content-addressed pattern.
- **Write amplification from speculative decoding**: branching factor 3–5× writes, most rejected immediately. Real per InstInfer/SpecInfer characterization.
- **Temporal profile**: Prefill = burst at full GPU compute rate (Llama-70B, 1K tokens ≈ 2.5 GB in 100–500ms). Decode = steady 20–50ms/token with growing read set. Read-dominated ~400:1 in token-accesses for a typical request.
- **Sharing ratios**: System prompts (200–2000 tokens) 100% shared. Few-shot (500–5000 tokens) shared per-application. SGLang/vLLM blogs imply >50% prefix hit in production chatbot workloads.
- **Selective retrieval matters**: InfiniGen (OSDI 2024) shows only 10–30% of KV entries are important per attention step → sparse random reads, not bulk.

### Still Open

- [ ] Exact block-level traces from production vLLM with PagedAttention (no public dataset exists)
- [ ] Quantitative sharing ratio distributions from BurstGPT traces (paper has arrivals, not prefix overlap)
- [ ] How does KV quantization (FP8/INT4) change IO sizes in practice vs theoretical 2–4× reduction?

### Key Sources

| System | Venue | Relevance |
|--------|-------|-----------|
| FlexGen | ICML 2023 | SSD offload scheduling (LP zig-zag), throughput-oriented |
| InstInfer | ASPLOS 2024 | Computational storage, KV stays on flash, P2P attention |
| InfiniGen | OSDI 2024 | Selective KV retrieval, sparse reads (10–30% subset) |
| Mooncake | Kimi/Moonshot 2024 | Production disaggregated, GPU/CPU/SSD tiers, KV-centric scheduler |
| DistServe/Splitwise | OSDI 2024 | Disaggregated prefill-decode, storage as cross-node channel |
| Sarathi-Serve | 2024 | Chunked prefill, temporal interleaving pattern |
| BurstGPT | Azure traces | 10.3M requests, 213 days, arrival patterns (not IO) |
| SpecInfer | 2024 | Tree-based speculative decoding, branching factor IO impact |
| Xinnor/MLPerf Storage 3.0 | 2025 | Measured IO sizes, bandwidth, queue depth, eviction policy comparison |

### Explicitly Out of Scope (not KV cache storage IO)

These affect inference performance but do NOT produce different certus Store/Load patterns:
- **MoE expert weight reads** — weight loading, not KV cache; may contend on NVMe bandwidth
- **Model/adapter switching** — weight warm-up, namespace turnover; not KV IO
- **Structured generation** (constrained decoding) — changes token count/latency, not IO direction
- **KV cache quantization** (FP8/INT4) — reduces block size (2× or 4× smaller), but same Store/Load pattern
- **GQA/MQA** — changes block size calculation (fewer KV heads), but same Store/Load pattern
- **Multimodal vision tokens** — large prefill burst (many tokens from image), but same Store/Load pattern as text prefill

---

## 10. BENCH_TARGET (vLLM Multi-Turn ShareGPT Replay) → Pattern Mapping

**What it is**: Real vLLM inference via the Certus gRPC connector, replaying 450 ShareGPT conversations × 12 turns. This is NOT a synthetic bench — it's actual model inference with real KV cache offloading through the full stack (vLLM → Python connector → gRPC → Rust server → SPDK NVMe).

**Dataset**: `sharegpt_12turn_450.json` — 450 real multi-turn conversations from ShareGPT, filtered to ≥2 human turns, capped at 12 rounds.

**Parameters**:
- 256 conversations (BENCH_TARGET) or 64 (firstrun), up to 450 max
- 12 turns per conversation
- 150 output tokens per generation
- MAX_NUM_SEQS=64 (concurrent batch size)
- Llama-3-8B, bf16, TP=1
- Slab size: 2 MiB (BENCH_TARGET) or 128 KiB (firstrun)
- Memory tier: 32 GiB (BENCH_TARGET) or 45 GiB (firstrun)
- `enable_prefix_caching=True` — RadixAttention active

**Inference patterns exercised**:

| Pattern | How it manifests | Coverage |
|---------|-----------------|----------|
| §1 Prefill writes | Each turn generates KV for the full accumulated context. Round 1 = 256 concurrent prefills, each writing prompt KV to certus | **Full** |
| §3 Decode phase | 150 output tokens per generation, each step reads all prior KV + appends 1 token | **Full** |
| §5 Shared prefix → unique | Conversations share nothing across each other (unique ShareGPT content), but WITHIN a conversation, turns share the accumulated prefix | **Partial** — intra-conversation sharing only, not cross-conversation cohort sharing |
| §6e Multi-turn reuse | **THE primary pattern** — each turn appends to accumulated context, so turn 12 re-reads ALL prior 11 turns' KV before generating | **Excellent** |
| §4 Eviction pressure | With 256 convs × growing context at 2 MiB slabs, working set eventually exceeds 32 GiB memory-tier → spilling to SSD | **Good** (firstrun with 45 GiB tier may not spill enough) |
| §6d Continuous batching | MAX_NUM_SEQS=64 with 256 conversations → vLLM schedules/preempts across conversations | **Full** |
| §6a Chunked prefill | vLLM 0.26 has chunked prefill — long accumulated contexts get chunked | **Implicit** (depends on vLLM scheduler) |

**What makes this workload unique compared to certus-api-bench v1/v2/v3**:
1. **Real growing contexts** — each turn's KV write is LARGER than the previous (accumulated history), unlike bench v1/v2/v3 where all blocks are uniform size
2. **Real scheduling pressure** — 256 conversations contending for 64 MAX_NUM_SEQS slots means preemption/reloading of KV
3. **Real prefix caching** — RadixAttention means vLLM can skip re-computing shared-prefix KV across turns of the same conversation
4. **Real eviction** — working set grows across rounds until memory-tier pressure triggers SSD offload/reload cycles
5. **IO accounting** — per-round `ssd_read`/`ssd_write` shows actual spilling behavior

**CONV_MULTIPLIER trick**: Setting `CONV_MULTIPLIER=4 MAX_ROUNDS=3` gives 1800 conversations over 3 rounds — 4× peak KV footprint. Each replica's turn-0 is uniquely tagged so KV blocks hash distinctly (no dedup). This forces MORE eviction pressure without needing more dataset.

**This benchmark is the ground truth** — certus-api-bench v1/v2/v3 are synthetic probes of individual sub-patterns. BENCH_TARGET exercises all patterns simultaneously with real model inference. The synthetic benches are useful for isolating specific bottlenecks (PCIe bandwidth, bidirectional contention, per-block latency) that are hard to measure in the full-stack vLLM run.

---

## 11. Synthetic Bench Versions → Inference Pattern Mapping

### certus-api-bench (v1)

**What it does**: Sequential populate → hot lookup → cold lookup. Single-threaded RPCs, per-iteration barriers between clients, no pipelining.

| Phase | Inference Pattern Mapping | Coverage |
|-------|--------------------------|----------|
| Populate (sequential batches) | §1 Prefill writes — but **serialized**, one batch at a time | Partial: misses concurrency of real prefill |
| Hot lookup (sequential) | §3 Decode reads (DRAM hit) — single request at a time | Partial: no interleave, no batching pressure |
| Cold lookup (SSD, per-iter barrier) | §4 Eviction reload — forced SSD reads after ClearMemoryTier | Partial: artificial (full cache clear), real eviction is gradual |

**What it stresses**: Baseline single-stream latency for each tier. Useful for measuring per-object overhead, not throughput.

**Not covered**: Concurrency, pipelining, read/write contention, sharing, fan-out.

---

### certus-api-bench_v2

**What it does**: Multi-client, pipelined RPCs (configurable `--pipeline-depth`), each client gets independent key range. Distinct GPU IPC buffers per key (forces real DMA per object).

| Phase | Inference Pattern Mapping | Coverage |
|-------|--------------------------|----------|
| Populate (batched, multi-client) | §1 Prefill writes — N concurrent prefill streams | **Good**: multiple clients writing simultaneously models concurrent prefill |
| Hot lookup (pipelined) | §3 Decode reads — pipelined RPCs saturate PCIe | **Good**: models vLLM's 48-thread reader pool hitting DRAM tier |
| Cold lookup (pipelined, unique keys) | §4 Eviction reload + §6f disaggregated decode | **Good**: pipelined SSD→GPU models real miss-on-decode or disaggregated load |
| Integrity verification | Correctness check (not a workload pattern) | N/A |

**What it stresses**: PCIe bandwidth saturation, multi-client throughput scaling, hot/cold tier transition correctness.

**Key knobs that map to inference parameters**:
- `--clients N` ≈ concurrent vLLM instances / prefill workers
- `--pipeline-depth` ≈ async IO depth (vLLM uses 48 reader threads)
- `--num-objects` ≈ blocks per request (prompt length / 16 tokens)
- `--block-size 4M` ≈ cross-layer KV block size (Llama-8B ≈ 2MB)

**Not covered**: Shared reads (cohort/prefix), bidirectional contention, per-block latency distribution, temporal mixing of prefill+decode.

---

### certus-api-bench_v3

**What it does**: Extends v2 with bidirectional phase (concurrent store+load), per-block latency isolation, and region-count sensitivity. Designed to measure CUDA stream overlap and cuMemcpyBatchAsync benefits.

| Phase | Inference Pattern Mapping | Coverage |
|-------|--------------------------|----------|
| Populate (Reserve→CopyToStore→Commit) | §1 Prefill writes — 3-phase commit protocol | **Good**: models write-once semantics with crash-consistent staging |
| Hot lookup (pipelined) | §3 Decode reads from DRAM | Same as v2 |
| **Bidirectional** | §4 + §6d Eviction writes concurrent with prefix loads | **Excellent**: directly models the real contention pattern — new prefills writing while decodes reading |
| Per-block latency (sequential) | §3 Single-token decode latency floor | **Good**: isolates DMA overhead per block (latency-critical decode path) |
| Cold lookup | §4 Miss-on-decode (SSD reload) | Same as v2 |

**What it stresses**: Bidirectional DMA overlap (measures load degradation under store pressure), driver call overhead (cuMemcpyBatchAsync vs individual calls), NonBlocking stream benefit.

**Key insight**: The `bidir_degradation` metric (load latency under store contention vs isolated) directly maps to "how much does decode latency suffer when prefills are running" — THE key metric for inference serving QoS.

**Not covered**: Prefix sharing / cohort reads, fan-out pattern, temporal session lifecycle, eviction policy behavior.

---

### Gap Analysis: What a v4 / workload_gen_agent Would Add

| Missing Pattern | v1 | v2 | v3 | Priority | Notes |
|----------------|----|----|----|----|------|
| §2 Cohort sharing (N readers, same blocks) | ❌ | ❌ | ❌ | **P0** | Core inference pattern; multiple clients reading same key set |
| §5 Shared prefix → unique suffix | ❌ | ❌ | ❌ | **P0** | Fan-out after shared phase; the hardest to model |
| §6a Chunked prefill (interleaved writes) | ❌ | ❌ | ❌ | **P1** | Gradual writes mixed with decode reads |
| §6d Continuous batching (temporal mix) | ❌ | ❌ | Partial (bidir) | **P1** | Realistic async scheduling interleave |
| §6e Multi-turn reuse (growing read set) | ❌ | ❌ | ❌ | **P1** | Incremental reload + append per turn |
| §6f Disaggregated (producer-consumer) | ❌ | Partial | ❌ | **P1** | Write-then-read with tail latency target |
| §6h Eviction under pressure (LRU, prefix-aware) | ❌ | Implicit | ❌ | **P1** | Working set > capacity, unique-suffix-first eviction |
| §6g Selective/sparse retrieval (10–30% subset) | ❌ | ❌ | ❌ | **P2** | InfiniGen-style sparse random reads |
| §6b Speculative decoding (write amplification) | ❌ | ❌ | ❌ | **P2** | 3–5× write with immediate discard |
| §6c Beam search (tree-structured COW) | ❌ | ❌ | ❌ | **P2** | Copy-on-write reads, diverge-on-write |

### Proposed Bench Version Roadmap

Each version isolates one pattern from real inference. Together they decompose what BENCH_TARGET does in aggregate.

| Bench | Pattern | What it isolates | Needs server? |
|-------|---------|-----------------|---------------|
| v1 (exists) | Baseline per-tier latency | Single-stream overhead floor | Yes |
| v2 (exists) | §1 Prefill + §4 Cold reload | Pipelined multi-client throughput | Yes |
| v3 (exists, fix/shared-queue) | §4+§6d Bidirectional | Store/load overlap, DMA stream efficiency | Yes |
| **v4** | §2+§5 Cohort sharing + fan-out | N readers on same keys → diverge into unique ranges | Yes |
| **v5** | §6e Multi-turn growth | Growing read set per turn (controlled version of BENCH_TARGET's primary pattern) | Yes |
| **v6** | §6a Chunked prefill interleave | Periodic writes mixed with decode reads, temporal mixing | Yes |
| **v7** | §6f Disaggregated pipeline | Bulk write → bulk read (producer-consumer, tail latency) | Yes |
| **v8** | §4+§6h Eviction under pressure | Gradual LRU with unique-suffix-first, working set > capacity | Yes |

**Relationship to BENCH_TARGET**: Each synthetic bench measures ONE dimension in isolation. When an optimization improves v4 (cohort reads) but regresses v3 (bidirectional), you know exactly what tradeoff you made. BENCH_TARGET gives the aggregate integration answer.

**Pattern generation is tractable** because each bench generates *storage-level commands* (Populate/Lookup with specific keys, timing, concurrency). The mapping from inference semantics → storage commands is well-defined in §1–§6 above.

---

### Reality Check: Single Client vs Multi Client

**In production with Certus, there is ONE gRPC client per vLLM worker process.**

```
vLLM Engine (1 process, TP=1)
  └── CertusGrpcOffloadingSpec (singleton per worker process)
       └── 1 gRPC channel → 1 stub (process-level singleton)
            └── certus-server (Rust, SPDK)
```

With TP>1, each GPU worker process gets its own spec instance → own gRPC channel:
- TP=1: **1 client** to the server
- TP=2: **2 clients** (one per GPU worker)
- TP=8: **8 clients**

Concurrency within that single client comes from a `ThreadPoolExecutor(max_workers=4)` in the handler + pipelined async gRPC futures. The v1 offloading API uses `cuMemcpyBatchAsync` to batch N block copies in one driver call.

**Implication for bench versions**: `--clients 1 --pipeline-depth 4` is the realistic single-engine scenario. `--clients 4+` simulates either TP>1 or multiple vLLM engines sharing one certus-server (multi-tenant).

---

### GPU Region Granularity: 64 KB per-layer vs 2 MB SSD slab

**The mismatch is real and important.**

| Layer | Granularity | What it represents |
|-------|-------------|-------------------|
| GPU KV cache (per-layer, vLLM 0.23+) | **~64 KB** | `block_size(16) × num_kv_heads(8) × head_dim(128) × dtype(2) × 2(K+V) = 65,536 bytes` per layer |
| GPU page (cross-layer, all layers) | **~2 MB** | All 32 layers packed: `64KB × 32 = 2,097,152 bytes` |
| Certus DRAM slot | **2 MB** (slab_size_bytes in BENCH_TARGET) | Server reserves one contiguous slot per block |
| Certus SSD extent | **2 MB** | One extent per block (same as DRAM slot) |
| MDTS segments | **128 KB** | NVMe max transfer size per command |

**How the scatter/gather works (vLLM 0.23+ per-layer layout → Certus):**

```
STORE (GPU→Server):
  Connector sends N=32 IPC handles per block (one per layer, each ~64 KB stride)
  Server opens each, copies 64KB × 32 into ONE contiguous 2 MB DRAM slot
  Background writer flushes the 2 MB slot to SSD as one extent

LOAD (Server→GPU):
  Server reads 2 MB from SSD → DRAM staging buffer
  Single-region (N==1): 1 × cudaMemcpyAsync(2MB, H2D) — optimal
  Multi-region (N>1):   scatter 2 MB → 32 × cudaMemcpyAsync(64KB, H2D)
                        each to the per-layer tensor at correct offset
```

**Performance implications per path:**

| Path | Operations | Driver calls | Bottleneck |
|------|-----------|-------------|-----------|
| Store (per-layer, N=32) | 32 × D2H 64KB | 32 (or 1 with cuMemcpyBatchAsync) | Driver call overhead |
| Load (per-layer, N=32) | 1 × SSD read 2MB + 32 × H2D 64KB | 1 + 32 (or 1+1 with batch) | Scatter overhead |
| Load (cross-layer, N=1) | 1 × SSD read 2MB + 1 × H2D 2MB | 2 | **Optimal** — no scatter |
| SSD I/O (either direction) | 2MB / 128KB MDTS = 16 segments | 16 NVMe commands | Sequential within one block |

**What this means for bench design:**
- certus-api-bench uses `--block-size 4M` with ONE IPC handle per block → models the **N==1 cross-layer** case (or batch-amortized case)
- Real vLLM 0.23+ sends **32 separate 64KB regions** per block → the server does 32 small cudaMemcpy calls per Load scatter
- `cuMemcpyBatchAsync` (CUDA 12.8+) batches all 32 × 64KB into ONE driver call → reduces overhead to near-N==1
- A future bench variant could model the per-region scatter by issuing 32 × 64KB lookups per logical block

---

### Store vs Load: When Each Happens in Real Inference

The two fundamental operations are **Store** (GPU→DRAM→SSD, "offload") and **Load** (SSD→DRAM→GPU, "reload"). Here's exactly when each fires:

#### STORE happens when:

| Trigger | Pattern | Size | Frequency |
|---------|---------|------|-----------|
| Prefill completes (new unique blocks only) | §1 | Unique suffix KV (M blocks) | Once per new request |
| Decode accumulates a full block | §3 | 1 block (16 tokens worth) | Every 16 decode steps |
| Chunked prefill chunk completes | §6a | 1 chunk (~512 tokens = 32 blocks) | Per chunk boundary |
| Speculative branches generated | §6b | Per-branch KV (discarded soon) | Per speculation step |
| Disaggregated: prefill node done | §6f | Entire request KV (GB-scale) | Once per request |

**Note**: Eviction (§4) does NOT trigger a Store. The SSD write already happened via the background writer when the block was originally Stored. Eviction just frees the DRAM slot.

#### LOAD happens when:

| Trigger | Pattern | Size | Frequency |
|---------|---------|------|-----------|
| Prefix cache hit (block in certus, not on GPU) | §1b, §2, §5 | Shared prefix blocks | Once per new request with certus hit |
| Preempted request rescheduled (KV was evicted from DRAM) | §3, §4 | Full request's KV blocks | On scheduler reschedule |
| New turn in multi-turn conversation (prior turns evicted) | §6e | ALL prior turns' KV | Once per new turn (if evicted) |
| Disaggregated: decode node starts | §6f | Entire request KV (GB-scale) | Once per request (always — different node) |
| Selective retrieval (sparse) | §6g | Subset of prior KV (10–30%) | Per decode step (if applicable) |

**Note**: All Loads except §6f require that the block was previously evicted from DRAM. If it's still in DRAM → Hot Load (fast, DRAM→GPU only). If evicted → Cold Load (slow, SSD→DRAM→GPU).

#### Temporal Interleave (what the server sees):

```
Time →

Steady state with 256 conversations, MAX_NUM_SEQS=64:

  STORE: ████░░████░░██░░████░░  (bursty: prefill completions + eviction)
  LOAD:  ░░██████░░██████░░████  (bursty: new turns + preemption reloads)
         ↑                       ↑
         prefill burst           eviction triggers
         writes KV               concurrent load of
                                 rescheduled requests
```

- **Early rounds** (turns 1–3): Store-dominated. All conversations prefilling, few evictions yet, small contexts.
- **Middle rounds** (turns 4–8): Mixed. Stores (growing prefills) + Loads (prior-turn reload, eviction reloads).
- **Late rounds** (turns 9–12): Load-dominated. Contexts large, heavy eviction pressure, every new turn reloads massive prior history.

#### Key insight for bench design:
- v1/v2 test Store and Load **sequentially** (populate phase, then lookup phase)
- v3 tests them **concurrently** (bidirectional phase)
- Real inference has them **interleaved with temporal correlation** — a Store (eviction) directly causes a future Load (when that evicted request gets rescheduled), and a Load (prefix hit) avoids a Store (no recompute needed)

---

## See Also

- [kv_IO_pattern.md](kv_IO_pattern.md) — Detailed block sizes, DMA patterns, read/write flows
- [kv_metadata_access.md](kv_metadata_access.md) — Metadata operation sizes and frequencies
- [vllm-offloading-connector-evolution.md](vllm-offloading-connector-evolution.md) — API evolution across vLLM versions

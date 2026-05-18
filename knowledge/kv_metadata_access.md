# KV Cache Metadata Access Patterns

Analysis of metadata operations across vLLM, llm-d-kv-cache, and 3FS for Certus design.

---

## 1. Metadata Access Size Summary

| System | Operation | Key/Data Size | Per-call I/O | Frequency |
|--------|-----------|---------------|--------------|-----------|
| vLLM (CPU offload) | `lookup(key)` | 36 bytes (32B hash + 4B group_idx) | 0 (in-memory dict) | Per-block, per-request, per-step |
| vLLM (CPU offload) | `touch(keys)` | N × 36B | 0 (OrderedDict reorder) | Per-request, per-step |
| llm-d fs_backend | `lookup(key)` | 36B → path string (~120 chars) | 1 `stat()` syscall (~4KB inode read) | Per-block, per-request |
| llm-d router index | `Lookup(keys, pods)` | N × 8B (uint64 BlockHash) | 0 (in-memory LRU) or 1 Redis RTT | Per-request at router |
| llm-d events | `BlockStoredEvent` | ~N×8B hashes + N×4B tokens + 8B parent + tier string | ~100-500 bytes msgpack per event | Per-block-store (async) |
| 3FS | `open()` → FDB txn | path resolution + inode (~256-512B) | 1 FDB network round-trip | Once per file (amortized) |
| 3FS | Data-path I/O | InodeId (8B) + offset → arithmetic | 0 (cached layout, no metadata I/O) | Per-chunk (zero metadata) |

---

## 2. vLLM Metadata Operations (Scheduler-Side, Critical Path)

### Block Hash Generation

- **Hash function**: SHA-256 (32 bytes output) or xxHash-128 (16 bytes), default SHA-256 via CBOR serialization
- **Input**: `(parent_block_hash, token_ids_tuple, extra_keys)` — Merkle chain
- **Output**: `BlockHash = bytes` (32 bytes with sha256_cbor, 16 bytes with xxhash_cbor)
- **OffloadKey**: `BlockHash || group_idx(4B big-endian)` = **36 bytes** (sha256) or **20 bytes** (xxhash)
- **Computed**: Incrementally per new full block of tokens — NOT on critical path

### Lookup (Hot Path)

```
Per new request per scheduling step:
  for each block in request.offload_keys:
    manager.lookup(key: 36 bytes) → bool|None
```

- **Data size per call**: 36 bytes key → boolean result
- **Call count**: N blocks × M new requests per step. Typical: 10-500 blocks/request × 1-64 requests/step
- **Latency budget**: Must complete within scheduling step (~1-5ms total budget)
- **Implementation (CPU offload)**: Python `OrderedDict.get()` — O(1) amortized, ~50-200ns per call
- **Implementation (shared storage)**: `os.path.exists(path)` — 1 `stat()` syscall, ~5-50µs on local NVMe, ~100-500µs on network FS

### Touch (Post-Scheduling)

- **What**: Moves N keys to MRU position in LRU/ARC
- **Data**: N × 36B keys
- **Latency**: Not on critical path — runs after scheduling decision made

### Prepare Load/Store (Scheduling)

- **What**: Pins/allocates slots, builds transfer spec
- **Output**: `GPULoadStoreSpec` = array of int64 block_ids + group_sizes + block_indices
- **Data flowing to worker**: ~8 bytes per block × N blocks per job

### Connector Metadata (Scheduler → Worker, per step)

```python
OffloadingConnectorMetadata:
  load_jobs: dict[int, TransferJob]   # job_id(8B) → (req_id(str) + TransferSpec)
  store_jobs: dict[int, TransferJob]  # same structure
  jobs_to_flush: set[int] | None
```

- **TransferSpec** = `(src_spec, dst_spec)` where each spec carries numpy int64 arrays of block_ids
- **Typical size per step**: ~100 bytes - 10KB depending on number of active transfers
- **Serialized**: Python pickle across process boundary (scheduler → worker)

### Worker → Scheduler Return

```python
OffloadingWorkerMetadata:
  completed_jobs: dict[int, int]  # job_id → completion_count
```

- **Size**: ~16 bytes per completed job × num_completed_this_step
- **Typical**: 0-10 completions per step = 0-160 bytes

---

## 3. llm-d Metadata Operations

### Engine-Side: SharedStorageOffloadingManager

| Operation | Data per call | I/O | Latency | Critical Path |
|-----------|--------------|-----|---------|---------------|
| `lookup(key)` | 36B key → `get_file_name()` → ~120 char path → `os.path.exists()` | 1 `stat()` | 5-50µs NVMe, 100-500µs NFS | YES |
| `prepare_load(keys)` | N keys → SharedStorageLoadStoreSpec (list of keys) | 0 | <1µs | YES |
| `prepare_store(keys)` | N keys → same | 0 | <1µs | Semi |
| `touch(keys)` | no-op | 0 | 0 | NO |
| `complete_load/store` | no-op | 0 | 0 | NO |

**Key bottleneck**: `os.path.exists()` per block. With 200 prefix blocks per request, that's 200 sequential `stat()` calls = 1-10ms on local NVMe, 20-100ms on shared storage.

### FileMapper: Hash → Path

```
<root>/<model_sha256[:12]>_r<rank>/<hash_hex[:3]>/<hash_hex[3:5]>_g<group_idx>/<full_hash_hex>.bin
```

- **Path computation**: Pure string ops, <1µs
- **Directory fanout**: 4096 × 256 × num_groups per rank
- **Implication**: Deep directory trees create inode lookup pressure on filesystem

### Router-Side: Go Index

| Operation | Key Size | Data Structure | Latency | Critical Path |
|-----------|----------|---------------|---------|---------------|
| `Lookup(N keys, P pods)` | N × 8B (uint64) | InMemory: golang-lru (50ns/get) | <100µs for 1000 keys | YES (routing) |
| `Lookup(N keys, P pods)` | N × 8B | Redis: pipelined HKEYS | 1-5ms (1 RTT) | YES (routing) |
| `Add(eng, req, pods)` | M × 8B + entries | InMemory: mutex + LRU.Add | <50µs | NO (async) |
| `Evict(key, pods)` | 8B + entries | InMemory: LRU.Remove | <10µs | NO (async) |

**Index entry size**:
- Key: `BlockHash` = `uint64` = 8 bytes
- Value per entry: `PodEntry` = PodIdentifier (string, ~32B) + DeviceTier (string, ~4B) + Speculative (bool) ≈ **~40 bytes per pod per key**
- With 10 pods per key: ~400 bytes per unique block in the index

**Index capacity** (InMemoryIndex default: 100M entries):
- At 400B/entry: ~40GB memory for full index
- CostAwareMemoryIndex (ristretto): capped at 2 GiB by default

### Event System: ZMQ Pub/Sub

| Event Type | Payload Size | Content |
|------------|-------------|---------|
| `BlockStoredEvent` | ~N×12B + overhead | N block hashes (8B each) + N token IDs (4B each) + parent hash (8B) + tier string |
| `BlockRemovedEvent` | ~N×8B + overhead | N block hashes + tier string |
| `AllBlocksClearedEvent` | ~10B | tier string only |

- **Transport**: 3-frame ZMQ message: `[topic, sequence(8B), msgpack_payload]`
- **Typical event**: 1-16 blocks stored at once → ~100-400 bytes per event
- **Rate**: Thousands of events/second across cluster
- **Latency to index**: Async — ZMQ delivery + parse + index write. Staleness window of 10-100ms typical.

---

## 4. 3FS Reference (for Future Disaggregated Architecture)

3FS is DeepSeek's production distributed filesystem used for KV cache at scale. Relevant only if Certus goes networked/disaggregated in a future phase.

### Key Design Patterns Worth Borrowing

1. **Zero metadata per I/O after setup**: After a one-time `open()` (which fetches the inode + layout), all data-path I/O requires zero metadata round-trips. Chunk placement is computed arithmetically from cached layout info. This is the target for any networked Certus: amortize metadata to connection setup, never per-request.

2. **USRBIO ring (io_uring-like shared-memory submission)**: Client submits I/O as 24-byte ring entries `{InodeId, offset, len}` into shared memory. FUSE daemon picks up batches and dispatches via RDMA. No syscalls on the data path. Pattern applicable if Certus adds a client library for remote access.

3. **Production KV cache performance**: 40 GiB/s peak read throughput per node (1×400Gbps NIC), with background GC running concurrently. Proves that NVMe + RDMA can saturate network bandwidth for this workload.

### What's NOT Relevant to Certus

- FDB schema / inode layout — Certus has no files/directories, just hash→location
- CRAQ replication protocol — Certus would use simpler replication for write-once blobs
- Failure recovery state machines — tied to CRAQ chain semantics
- POSIX semantics (dirs, links, sessions) — Certus is a flat content-addressed store

### 3FS Chunk Size Alignment

3FS chunk sizes: {512KB, 1MB, 2MB, 4MB, 16MB, 64MB}. KV cache offloaded blocks (32-80MB) don't align cleanly — most require multi-chunk or waste space. A purpose-built system (like Certus) with variable extent sizes avoids this mismatch.

---

## 5. Metadata Access Size Comparison (Bytes per Operation)

| Operation | vLLM CPU Offload | llm-d Shared Storage | llm-d Router Index | Certus |
|-----------|-----------------|---------------------|-------------------|--------|
| **Existence check** | 36B key → dict lookup (0 I/O) | 36B key → ~120B path → 4KB inode stat | 8B hash → dict lookup (0 I/O) | 8B key → HashMap (0 I/O) |
| **Single block lookup** | 36B | 36B + stat() | 8B | 8B |
| **N-block prefix check** | N × 36B | N × (36B + stat()) | N × 8B (one pipeline) | N × 8B (batch_check loop) |
| **Store notification** | N × 36B (dict insert) | N × ~120B path (write file) | N × (8B hash + ~40B PodEntry) msgpack | N × 8B (HashMap insert) |
| **Transfer metadata** | ~8B/block (int64 block_ids) | List of 36B OffloadKeys | N/A | 8B/block (gpu_block_ids) |
| **Eviction update** | 36B key (dict remove) | file delete by path | 8B hash + PodEntry remove | 8B key (HashMap remove) |

---

## 6. Certus Architecture: How It Already Handles Metadata

### Certus DispatchMap = The In-Memory Index

Certus **already solves the metadata bottleneck** via its `DispatchMap` component:

```
batch_check(keys: &[u64]) → count of consecutive hits
  └── dispatcher.check(key: u64) → Ok(true/false)
       └── DispatchMapState.inner.entries.get(&key) → Option<DispatchEntry>
            (HashMap<CacheKey, DispatchEntry>, Mutex-protected, purely in-memory)
```

- **CacheKey = u64** (8 bytes) — Python connector truncates OffloadKey to u64 before FFI
- **Lookup**: `HashMap.get()` — O(1), ~50ns, zero I/O
- **No stat() calls, no filesystem, no path resolution**

### Metadata NVMe Device (metadata_pci_addr)

The "metadata NVMe" in Certus config is for the **ExtentManager** — persisting allocation bitmap and extent records for crash consistency. It is NOT used in the hot-path lookup. The per-request `batch_check()` never touches NVMe.

```
Hot path (per-step, per-block):     HashMap lookup → ~50ns, 0 I/O
Cold path (on store/evict):         ExtentManager → metadata NVMe (crash record)
Background (flush):                 Staging DRAM → Data NVMe (async)
```

### How Certus Compares

| Operation | llm-d (filesystem) | Certus (already built) |
|-----------|--------------------|-----------------------|
| Existence check | `os.path.exists()` → 5-500µs | `HashMap.get(u64)` → ~50ns |
| Key size | 36B (sha256 + group_idx) | 8B (u64) |
| Path resolution | 3-level dir fanout + dentry cache | None — direct hash lookup |
| Per-step overhead (200 blocks) | 200 × stat() = 1-10ms | 200 × HashMap get = ~10µs |
| Crash consistency | Atomic rename per file | ExtentManager bitmap on metadata NVMe |

### What Certus Does NOT Yet Handle (vs llm-d/3FS)

1. **Cluster-wide event propagation**: llm-d's ZMQ pub/sub notifies the router index when blocks are stored/evicted. Certus is currently single-node — no event publishing to cluster. *(Multi-node only)*

2. **Router-side index**: The Go-side `InMemoryIndex` that answers "which pods have this block?" doesn't exist in Certus. If Certus becomes multi-node, it needs an equivalent. *(Multi-node only)*

3. **3FS-style RDMA data path**: 3FS uses RDMA for data transfer; Certus uses SPDK NVMe→DRAM→GPU. Different network topology assumption. *(Future disaggregated tier only)*

**Note**: Eviction (watermark-based with ref-counting/pin semantics) and job completion tracking (`poll_completions()` returning `(job_id, success)` pairs) are already implemented. The main single-node gap is that I/O is currently synchronous per-lookup rather than fully async with high queue depth.

### Metadata I/O Size vs Data I/O Size

| I/O Type | Access Size | Frequency | Critical Path | Where in Certus |
|----------|-------------|-----------|---------------|-----------------|
| Metadata (check) | 8B key → in-memory | 100-10000/step | YES | DispatchMap HashMap |
| Metadata (persist) | 4KB extent record | Per store/evict | NO (async) | metadata NVMe |
| Metadata (events) | 100-500B per event | 1000s/sec cluster | NO | Not yet implemented |
| **Data (read)** | **4-80 MB per blob** | **1-48 concurrent** | **YES** | Data NVMe → GPU (P2P) |
| **Data (write)** | **4-80 MB per blob** | **1-16 concurrent** | **NO (deferred)** | GPU → DRAM staging → NVMe |

**Key insight**: Certus eliminates the metadata latency problem entirely for local operations. The remaining metadata challenge is cluster-wide coordination (event propagation, multi-node routing) which is a different problem from per-I/O existence checks.

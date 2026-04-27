# Design for the cache 'put' direction

## Overview

The put flow moves a GPU tensor (cache block) from GPU memory through a DRAM staging area and ultimately to SSD. This is a **cache** — the source of truth lives elsewhere (e.g., model weights, upstream store), so data loss on crash is acceptable. On restart, the hash table is rebuilt by iterating over finalized extents in the extent manager.

## Assumptions and Invariants

- **Cache block sizes are fixed per model, variable across models.** Each model served has a fixed cache block size, but different models may use different sizes concurrently. The staging buffer pool must support multiple size classes, extent allocation accommodates variable-size requests, and the marker block records the block's actual size. The system handles multiple block sizes simultaneously.
- **Single dispatcher process.** One Request Dispatcher process handles all client requests. No sharding or multi-instance coordination.
- **No ordering guarantees across keys.** Puts to different keys are fully independent and may complete (reach SSD) in any order. Puts to the same key follow last-writer-wins semantics (see step 3 below).
- **Cache semantics.** DRAM-staged data is volatile. A crash between steps 3–7 loses the block; this is acceptable because the data is recoverable from the original source.
- **Staging pool sizing.** The number of staging buffers must be large enough to absorb burst put rates while the SSD flush pipeline drains. As a guideline, the pool should hold at least `(sustained_put_rate × average_flush_latency)` buffers to avoid chronic back-pressure. Under-provisioning leads to frequent client timeouts; over-provisioning wastes DRAM.

## Put Flow

1. **Client submits request via gRPC.** The client synchronously passes an IPC handle for a GPU tensor (cache block), together with a key, to the Request Dispatcher in a separate process.

2. **Dispatcher allocates a staging buffer and initiates GPU DMA.** The dispatcher allocates a staging buffer from a **pre-allocated DRAM pool** and performs a CUDA DMA (`cudaMemcpyAsync`) from the GPU to the staging buffer using the IPC handle. If no staging buffers are available, the dispatcher applies **back-pressure** — the gRPC call blocks until a buffer is freed, up to a **configurable timeout**. If the timeout expires, the dispatcher returns a `RESOURCE_EXHAUSTED` error to the client, allowing it to retry or take alternative action rather than stalling indefinitely.

3. **Hash table is updated to point to DRAM.** When the DMA to the staging buffer completes, the dispatcher atomically registers the cache block in the hash table, mapping the key to the DRAM staging buffer. The hash table entry has **two states**: it either points to a DRAM staging buffer or to an SSD offset. There is no intermediate "flushing" state. **Duplicate put (same key):** if a put arrives for a key that is already staged or being flushed, the new value replaces the old one in DRAM. Any in-flight SSD flush for the old value is **logically abandoned** — the DMA is allowed to run to completion (no cancellation), but its result is discarded. The old extent is freed only **after** the in-flight DMA finishes, to prevent freeing storage that is still a DMA target. The old DRAM staging buffer is **ref-counted** (same mechanism as step 9): it is returned to the pool only when the last concurrent reader drops its reference, preventing use-after-free for any in-flight `get` still reading the old buffer.

4. **Client receives acknowledgement.** Once the hash table is updated, the cache block is available for `get` requests (served from DRAM). An acknowledgement is returned to the client via the gRPC response.

5. **Extent allocation (async).** Asynchronously, the dispatcher allocates a **single contiguous extent** on the SSD via the extent manager. The extent size covers the cache block data plus a tail-end marker block. **SSD full:** if the extent manager cannot allocate, an **eviction** of existing SSD-resident extents is triggered to free space before retrying. **Eviction policy:** lowest-priority extents are evicted first; within the same priority level, least-recently-used (LRU) extents are evicted first. When an extent is evicted, its hash table entry is **atomically removed** so that no `get` request can chase a dangling SSD offset.

6. **DMA from DRAM staging to SSD.** The dispatcher triggers a DMA copy from the staging buffer to the allocated SSD offset. **DMA failure handling:** an SSD write failure or timeout is retried a **bounded number of times** (configurable). If retries are exhausted, the failing SSD is **taken offline** — its extents are invalidated in the hash table, and the extent manager marks the device as unavailable. The dispatcher continues operating with remaining healthy devices. The failed put's staging buffer is released, and the put is re-attempted on an alternate device if one is available.

7. **Marker block write and extent finalization.** On completion of the DMA:
   - The **marker block** is written to the tail end of the extent on SSD. The marker is a **full metadata record** containing the key, data size, checksum, timestamp, and other metadata needed to validate integrity and support recovery. The marker **must fit within a single 4KiB block** to guarantee atomic writes — a torn marker write is prevented by the device's atomic write unit.
   - After the marker block write is **confirmed durable** (flushed to media), the extent is **marked as finalized** in the extent manager via a separate metadata update. **This ordering is mandatory** — the marker must be on stable storage before the finalize bit is persisted, so that recovery never encounters a finalized extent with a missing or corrupt marker. The finalized state is **persistent** — it survives crashes. On recovery, finalized extents are considered occupied; non-finalized extents are reclaimed as free space.
   - Extents that are not finalized are cleaned up after a certain period of time.

8. **Hash table atomically updated to SSD.** The hash table entry is atomically swapped from the DRAM staging pointer to the SSD offset. After this point, `get` requests for this key are served from SSD.

9. **Staging buffer released.** The staging buffer is returned to the pre-allocated pool. Staging buffers are **reference-counted**: the buffer is only freed when the last reader (any concurrent `get` still reading from DRAM) drops its reference. This prevents use-after-free races between the flush completing and in-flight DRAM reads.

## Get Interaction During Flush

A `get` request is served from whichever location the hash table entry currently points to:
- **Before step 8:** served from DRAM staging buffer.
- **After step 8:** served from SSD.

The atomic swap in step 8 is the transition point. Readers that obtained a DRAM pointer before the swap continue reading from DRAM safely (reference counting prevents premature buffer release).

## Crash Recovery

On restart, the in-memory hash table is empty. It is rebuilt by **iterating over the extent manager's finalized extents**. Each finalized extent contains a marker block with the key, offset, and size (amongst other metadata such as priority). Non-finalized extents (from incomplete writes) are reclaimed as free space. **Duplicate keys:** if multiple finalized extents contain the same key (possible if the system crashed before step 8 completed), **all duplicates are reclaimed** — neither is inserted into the hash table. Since access timestamps are not available in extent metadata to determine which is authoritative, and serving stale data is worse than a cache miss, the safe choice under cache semantics is to discard both and let the client re-fetch from the source of truth. DRAM-only entries (staged but not yet flushed) are lost, which is acceptable under cache semantics.

## Observability

The following metrics should be instrumented to support monitoring and capacity planning:

- **Staging pool utilization** — gauge of in-use vs. total staging buffers. High utilization signals back-pressure risk.
- **Back-pressure timeouts** — counter of `RESOURCE_EXHAUSTED` errors returned to clients due to staging pool exhaustion.
- **GPU DMA latency** — histogram of CUDA DMA transfer times (step 2).
- **SSD flush latency** — histogram of end-to-end time from extent allocation to finalization (steps 5–7).
- **SSD DMA latency** — histogram of DRAM-to-SSD DMA transfer times (step 6).
- **Eviction rate** — counter of extents evicted to free SSD space.
- **Eviction latency** — histogram of time spent in eviction before a new extent can be allocated.
- **Put throughput** — counter of successful puts per second (acknowledgements sent, step 4).
- **Hash table size** — gauge of entries in the hash table, broken down by state (DRAM vs. SSD).
- **Recovery duration** — time taken to rebuild the hash table from finalized extents on restart.

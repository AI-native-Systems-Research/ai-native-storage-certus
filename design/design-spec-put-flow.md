# Design for the cache 'put' direction

## Overview

The put flow moves a GPU tensor (cache block) from GPU memory through a DRAM staging area and ultimately to SSD. This is a **cache** — the source of truth lives elsewhere (e.g., model weights, upstream store), so data loss on crash is acceptable. On restart, the hash table is rebuilt by iterating over finalized extents in the extent manager.

## Assumptions and Invariants

- **Cache blocks are fixed-size.** The staging buffer pool, extent allocation, and marker block layout all assume a single fixed block size (configured at startup).
- **Single dispatcher process.** One Request Dispatcher process handles all client requests. No sharding or multi-instance coordination.
- **No ordering guarantees across keys.** Puts to different keys are fully independent and may complete (reach SSD) in any order. Puts to the same key follow last-writer-wins semantics (see step 3 below).
- **Cache semantics.** DRAM-staged data is volatile. A crash between steps 3–7 loses the block; this is acceptable because the data is recoverable from the original source.

## Put Flow

1. **Client submits request via gRPC.** The client synchronously passes an IPC handle for a GPU tensor (cache block), together with a key, to the Request Dispatcher in a separate process.

2. **Dispatcher allocates a staging buffer and initiates GPU DMA.** The dispatcher allocates a staging buffer from a **pre-allocated DRAM pool** and performs a CUDA DMA (`cudaMemcpyAsync`) from the GPU to the staging buffer using the IPC handle. If no staging buffers are available, the dispatcher applies **back-pressure** — the gRPC call blocks until a buffer is freed.

3. **Hash table is updated to point to DRAM.** When the DMA to the staging buffer completes, the dispatcher atomically registers the cache block in the hash table, mapping the key to the DRAM staging buffer. The hash table entry has **two states**: it either points to a DRAM staging buffer or to an SSD offset. There is no intermediate "flushing" state. **Duplicate put (same key):** if a put arrives for a key that is already staged or being flushed, the new value replaces the old one in DRAM. Any in-flight SSD flush for the old value is abandoned and the old extent, if allocated, is freed.

4. **Client receives acknowledgement.** Once the hash table is updated, the cache block is available for `get` requests (served from DRAM). An acknowledgement is returned to the client via the gRPC response.

5. **Extent allocation (async).** Asynchronously, the dispatcher allocates a **single contiguous extent** on the SSD via the extent manager. The extent size covers the cache block data plus a tail-end marker block. **SSD full:** if the extent manager cannot allocate, an **eviction** of existing SSD-resident extents is triggered to free space before retrying.

6. **DMA from DRAM staging to SSD.** The dispatcher triggers a DMA copy from the staging buffer to the allocated SSD offset. **DMA failure is fatal** — an SSD write failure or timeout is treated as a hardware fault; the dispatcher logs the error.

7. **Marker block write and extent finalization.** On completion of the DMA:
   - The **marker block** is written to the tail end of the extent on SSD. The marker is a **full metadata record** containing the key, data size, checksum, timestamp, and other metadata needed to validate integrity and support recovery.
   - After the marker block write completes, the extent is **marked as finalized** in the extent manager via a separate metadata update. The finalized state is **persistent** — it survives crashes. On recovery, finalized extents are considered occupied; non-finalized extents are reclaimed as free space.
   - Extents that are not finalized are cleaned up after a certain period of time.

8. **Hash table atomically updated to SSD.** The hash table entry is atomically swapped from the DRAM staging pointer to the SSD offset. After this point, `get` requests for this key are served from SSD.

9. **Staging buffer released.** The staging buffer is returned to the pre-allocated pool. Staging buffers are **reference-counted**: the buffer is only freed when the last reader (any concurrent `get` still reading from DRAM) drops its reference. This prevents use-after-free races between the flush completing and in-flight DRAM reads.

## Get Interaction During Flush

A `get` request is served from whichever location the hash table entry currently points to:
- **Before step 8:** served from DRAM staging buffer.
- **After step 8:** served from SSD.

The atomic swap in step 8 is the transition point. Readers that obtained a DRAM pointer before the swap continue reading from DRAM safely (reference counting prevents premature buffer release).

## Crash Recovery

On restart, the in-memory hash table is empty. It is rebuilt by **iterating over the extent manager's finalized extents**. Each finalized extent contains a marker block with the key, offset, and size (amongst other metadata such as ownership id, priority). Non-finalized extents (from incomplete writes) are reclaimed as free space. DRAM-only entries (staged but not yet flushed) are lost, which is acceptable under cache semantics.

# Design for the cache 'get' / 'hit' direction

## Overview

The get/hit flow serves a cached block from either DRAM (staging) or SSD into client-provided GPU memory. The hash table entry determines the data source: if the block is still DRAM-resident (not yet flushed), the transfer is from DRAM; if SSD-resident, the transfer uses peer-to-peer DMA for a direct SSD → GPU path.

## Assumptions and Invariants

- **Fixed-size cache blocks.** The client must provide a size matching the stored block size; mismatches are rejected.
- **Reference counting prevents mutation.** While any get is in-flight for a key, both puts and evictions for that key are blocked until all readers release.
- **Concurrent gets are parallel.** Multiple clients can issue gets for the same key simultaneously. Each increments the ref count independently and triggers its own DMA transfer.
- **Cache miss is client-handled.** On a miss, the dispatcher returns immediately; there is no transparent fill-from-source.
- **DMA failure is fatal.** A failed or timed-out DMA transfer (SSD → GPU or DRAM → GPU) is treated as a hardware fault; the dispatcher logs and shuts down.

## Get/Hit Flow

1. **Client submits get request via gRPC.** The client sends the key, an IPC handle for a pre-allocated GPU tensor (destination), and the expected cache-block size.

2. **Dispatcher looks up key.** The dispatcher looks up the key in the hash table. 

3. **Cache miss → immediate return.** If the key is not present, the dispatcher returns a miss response to the client. The client is responsible for fetching data from the original source and optionally issuing a put.

4. **Cache hit → increment ref count.** If the key is present, the dispatcher first validates the size, then atomically increments the reference count on the hash table entry. This prevents concurrent puts (which would replace the entry) and evictions (which would free the extent) while any transfer is in progress.

5. **DMA transfer to GPU.** The dispatcher initiates a DMA transfer into the client's GPU memory using the IPC handle:
   - **DRAM-resident entry:** Data is copied from the DRAM staging buffer to GPU memory (DRAM → GPU DMA).
   - **SSD-resident entry:** Data is transferred directly from the SSD to GPU memory via **Peer to peer DMA** (SSD → GPU, no intermediate DRAM bounce buffer).

6. **Release ref count and acknowledge.** When the DMA transfer completes, the reference count is decremented. Once the ref count reaches zero, the entry becomes eligible for puts and evictions again. An acknowledgement (hit response) is sent back to the client via gRPC.

## Interaction with Put and Eviction

- **Concurrent put for a ref-counted key:** The put is blocked (deferred) until all in-flight transfers complete and the ref count drops to zero. This ensures readers always see consistent data during transfer.
- **Eviction:** The hash-table value includes a last access timestamp and a priority level (SLO). An eviction process uses this information to evict elements from the cache.
- **Eviction of a ref-counted key:** The evictor skips entries with non-zero ref counts and selects a different key to evict. There is no deferred eviction queue.
- **Hash table state transition during get:** If the async flush (from the put flow) completes while a DRAM-resident get is in progress, the reader continues from DRAM safely — it already holds a reference. The hash table entry swaps to SSD, but the staging buffer is not freed until the ref count reaches zero.

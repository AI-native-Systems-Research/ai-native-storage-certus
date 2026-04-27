# Design for the cache 'get' / 'hit' direction

## Overview

The get/hit flow serves a cached block from either DRAM (staging) or SSD into client-provided GPU memory. The hash table entry determines the data source: if the block is still DRAM-resident (not yet flushed), the transfer is from DRAM; if SSD-resident, the transfer uses peer-to-peer DMA for a direct SSD → GPU path.

## Assumptions and Invariants

- **Cache block sizes are fixed per model, variable across models.** Each model has a fixed cache block size, but different models may use different sizes concurrently. The client must provide an expected size matching the stored block's actual size (recorded in the hash table entry); mismatches are rejected.
- **Reference counting protects in-flight reads.** While a get is in-flight, the reader holds a reference to the current data location (DRAM buffer or SSD extent). A concurrent put **replaces** the hash table entry immediately — the old data location is ref-counted and freed only when all in-flight readers release. Evictions skip entries with non-zero ref counts (see below).
- **Concurrent gets are parallel.** Multiple clients can issue gets for the same key simultaneously. Each increments the ref count independently and triggers its own DMA transfer.
- **Cache miss is client-handled.** On a miss, the dispatcher returns immediately; there is no transparent fill-from-source.
- **DMA failure handling.** A failed or timed-out DMA transfer is retried a **bounded number of times** (configurable). If retries are exhausted: for **SSD → GPU** failures, the failing SSD is **taken offline** (same as put flow) and an error is returned to the client. For **DRAM → GPU** failures, an error is returned to the client. In both cases the reference count is decremented so the entry is not permanently locked.

## Get/Hit Flow

1. **Client submits get request via gRPC.** The client sends the key, an IPC handle for a pre-allocated GPU tensor (destination), and the expected cache-block size. The client determines the expected size from its own model configuration — this is a safety check to confirm the client and cache agree on the block size.

2. **Dispatcher looks up key.** The dispatcher looks up the key in the hash table. 

3. **Cache miss → immediate return.** If the key is not present, the dispatcher returns a miss response to the client. The client is responsible for fetching data from the original source and optionally issuing a put.

4. **Cache hit → increment ref count.** If the key is present, the dispatcher first validates the size, then atomically increments the reference count on the data location (DRAM buffer or SSD extent). This ensures the data location is not freed while the transfer is in progress. Concurrent puts may still **replace** the hash table entry — the reader continues from the old location via its held reference. Evictions skip entries with non-zero ref counts.

5. **DMA transfer to GPU.** The dispatcher initiates a DMA transfer into the client's GPU memory using the IPC handle. A **configurable timeout** is applied to the transfer — if exceeded, the transfer is treated as a DMA failure (see retry/offline handling above) and the ref count is decremented.
   - **DRAM-resident entry:** Data is copied from the DRAM staging buffer to GPU memory (DRAM → GPU DMA).
   - **SSD-resident entry:** Data is transferred directly from the SSD to GPU memory via **Peer to peer DMA** (SSD → GPU, no intermediate DRAM bounce buffer). **P2P DMA is a hard requirement** — the hardware topology must support direct SSD→GPU transfers (appropriate PCIe topology, IOMMU configuration, and device capabilities). The dispatcher validates P2P capability at startup and refuses to start if it is not available.

6. **Release ref count and acknowledge.** When the DMA transfer completes, the reference count is decremented. Once the ref count reaches zero, the entry becomes eligible for puts and evictions again. An acknowledgement (hit response) is sent back to the client via gRPC.

## Interaction with Put and Eviction

- **Concurrent put for a ref-counted key:** The put **replaces** the hash table entry immediately with the new DRAM staging buffer. In-flight readers continue from the old data location (DRAM buffer or SSD extent) using the reference they already hold. The old data location is freed only when the last reader releases its reference. Readers may serve the prior value for the duration of their in-flight DMA — this is acceptable under cache semantics.
- **Eviction:** The hash table entry tracks a **last-access timestamp** (volatile, in-memory only — not persisted to SSD) and a **priority level** (from the marker block). The eviction policy selects lowest-priority extents first; within the same priority, least-recently-used (by last-access timestamp) extents are evicted first. After crash recovery, all entries start with no access history; eviction falls back to **priority-only** ordering until the cache warms up.
- **Eviction of a ref-counted key:** The evictor skips entries with non-zero ref counts and selects a different key to evict. There is no deferred eviction queue.
- **Hash table state transition during get:** If the async flush (from the put flow) completes while a DRAM-resident get is in progress, the reader continues from DRAM safely — it already holds a reference. The hash table entry swaps to SSD, but the staging buffer is not freed until the ref count reaches zero.

## Observability

The following metrics should be instrumented to support monitoring and performance analysis:

- **Hit/miss ratio** — counter of cache hits vs. misses. The primary indicator of cache effectiveness.
- **Get latency (DRAM)** — histogram of end-to-end get latency for DRAM-resident entries.
- **Get latency (SSD)** — histogram of end-to-end get latency for SSD-resident entries (P2P DMA path).
- **DMA transfer latency** — histogram of raw DMA transfer times, broken down by path (DRAM → GPU vs. SSD → GPU).
- **DMA retries** — counter of DMA transfer retries, broken down by path.
- **DMA failures** — counter of DMA transfers that exhausted retries, broken down by path.
- **Ref-count high-water mark** — gauge of the maximum concurrent ref count observed per entry. High values indicate hot keys.
- **Size mismatch rejections** — counter of get requests rejected due to block size mismatch.
- **Get throughput** — counter of successful get completions per second.

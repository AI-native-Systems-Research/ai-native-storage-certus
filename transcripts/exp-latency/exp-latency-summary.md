What are the limitations of this new version, e.g. poor multi-client scaling?

  Thought for 4s

Good question. Here are the key limitations and trade-offs of the current exp/latency branch:

1. Multi-client scaling with block_in_place

The biggest concern. block_in_place runs blocking work directly on Tokio worker threads (of which we now have only 4). Under high concurrency (e.g. 8+ clients issuing simultaneous lookups), all 4 workers can be blocked in dispatcher calls, starving the async runtime — gRPC can't even accept new connections or parse requests until a worker frees up. spawn_blocking avoided this by using a separate pool (up to 512 threads). With 4+ concurrent clients doing cold-path lookups (which take >1ms), this will bottleneck.

2. Busy-poll NVMe actor burns a full CPU core

ActorParkConfig::busy_poll() means the NVMe actor thread never yields. With 4 drives that's 4 cores at 100% even when idle. Fine for a dedicated storage server, but wasteful in shared environments or when the system is lightly loaded.

3. NUMA pinning is hardcoded to CPUs 0-15

The on_thread_start pins to CpuSet::from_cpus(0..16) regardless of actual NUMA topology. On systems where node 0 has different CPU ranges (e.g. hyperthreaded systems with 0-31 on node 0), or where the GPU/NVMe are on node 1, this is counterproductive.

4. Dispatch-map sharding doesn't help single-key contention

16 shards eliminate cross-key contention, but if multiple clients write and read the same key simultaneously, they still serialize on the same shard's mutex + condvar. The write-ref blocking pattern is unchanged.

5. PIPELINE_RING_SIZE=4 may hurt throughput under heavy batch load

More frequent GPU stream syncs (every 4 vs 8 completions) add sync overhead. For large multi-object cold batches (32+ objects per drive), this means ~2x as many synchronization points, potentially reducing sustained cold-path throughput even as it improves single-request latency.

6. DashMap has higher per-operation overhead than Mutex for low contention

Under single-client (the benchmark case), DashMap's internal sharding + hashing is actually slightly more expensive than an uncontended Mutex::lock(). The benefit only appears with concurrent IPC handle lookups from multiple clients.

7. No warm-up or adaptive behavior

The system doesn't adapt between latency-optimized and throughput-optimized modes. A production system might want to detect load level and switch (e.g. park actors under low load to save power, busy-poll under high load for responsiveness).

---
Most impactful fix for production would be: replace block_in_place with a bounded dedicated thread pool (e.g. 8-16 threads, pinned to NUMA-0) instead of relying on Tokio's 4 workers. That preserves the "no thread-pool handoff" benefit for low concurrency while scaling under load.

//! Registered buffer pool for serving block-tier entries.
//!
//! Buffers are allocated with `spdk_zmalloc` on the serving instance's NUMA node
//! and registered with `ibv_reg_mr` — both, because the device read needs
//! SPDK-DMA-able memory and the RDMA needs a registered region. The memory tier
//! is already dual-registered exactly this way, so no new mechanism is needed
//! (research D15).
//!
//! A block-tier entry is served from here and is **never** promoted into the
//! source's memory tier, so serving a peer does not evict the source's own
//! working set (FR-012, research D8).
//!
//! Scaffolding: lands in T037.

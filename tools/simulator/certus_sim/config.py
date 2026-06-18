from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class SimConfig:
    """All simulation parameters: capacity, timing, and pipeline configuration."""

    # --- Drive topology ---
    num_drives: int = 4
    drive_capacity_bytes: int = 1_000_000_000_000  # 1 TB per drive

    # --- Memory tier ---
    memory_tier_capacity_bytes: int = 2 * 1024**3  # 2 GiB
    memory_tier_shards: int = 16

    # --- Entry size (uniform for simplicity) ---
    entry_size_bytes: int = 131072  # 128 KiB default

    # --- Eviction ---
    max_eviction_attempts: int = 2048
    ssd_eviction_threshold: float = 0.9
    ssd_eviction_low_watermark: float = 0.8
    ssd_eviction_batch_size: int = 64
    ssd_eviction_interval_us: float = 5_000_000.0  # 5 seconds

    # --- Timing (microseconds) ---
    gpu_d2h_latency_us: float = 50.0
    gpu_h2d_latency_us: float = 40.0
    nvme_read_latency_us: float = 80.0  # per MDTS segment
    nvme_write_latency_us: float = 20.0  # per MDTS segment
    ipc_open_latency_us: float = 100.0
    ipc_close_latency_us: float = 10.0
    memory_tier_insert_us: float = 1.0
    dispatch_map_op_us: float = 0.5
    grpc_overhead_us: float = 50.0

    # --- Pipeline ---
    mdts_bytes: int = 131072  # 128 KiB max NVMe transfer size
    max_queue_depth: int = 16
    max_queues_per_drive: int = 2

    # --- Write-through ---
    write_through_enabled: bool = True

    # --- Derived helpers ---
    def segments_per_entry(self) -> int:
        return max(1, (self.entry_size_bytes + self.mdts_bytes - 1) // self.mdts_bytes)

    def drive_for_key(self, key: int) -> int:
        return key % self.num_drives

    def shard_for_key(self, key: int) -> int:
        return key % self.memory_tier_shards

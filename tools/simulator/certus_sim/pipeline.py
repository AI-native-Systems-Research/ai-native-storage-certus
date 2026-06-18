"""Pipelined SSD→DRAM→GPU reader timing model.

Models the sliding-window zero-copy pipeline from the spec:
- Object is divided into MDTS-sized segments
- Up to max_queue_depth NVMe reads in flight
- As each read completes, GPU H2D copy is issued immediately
- Overlaps NVMe I/O with GPU DMA for minimum wall-clock time
"""

from __future__ import annotations

from certus_sim.config import SimConfig
from certus_sim.gpu_dma import GpuDmaModel


class PipelineModel:
    """Computes cold promotion latency using the pipelined reader model."""

    def __init__(self, config: SimConfig, gpu: GpuDmaModel):
        self.config = config
        self.gpu = gpu

    def single_entry_promote_latency(self, size_bytes: int) -> float:
        """Compute total cold promotion latency for a single entry.

        Models the sliding-window pipeline:
        - N segments, each requiring one NVMe read + one GPU H2D copy
        - Pipeline depth = max_queue_depth
        - NVMe reads and GPU copies overlap after initial fill
        """
        num_segments = max(1, (size_bytes + self.config.mdts_bytes - 1) // self.config.mdts_bytes)
        segment_size = min(size_bytes, self.config.mdts_bytes)

        nvme_read_time = self.config.nvme_read_latency_us
        gpu_copy_time = self.gpu.h2d_latency(segment_size)
        depth = min(self.config.max_queue_depth, num_segments)

        # Pipeline model:
        # First segment: nvme_read + gpu_copy (no overlap yet)
        # Remaining segments: max(nvme_read, gpu_copy) each (fully overlapped)
        if num_segments == 1:
            return nvme_read_time + gpu_copy_time

        # Fill phase: first 'depth' reads are submitted
        fill_time = nvme_read_time  # first read completes
        # Steady state: each subsequent segment takes max(read, copy)
        steady_per_segment = max(nvme_read_time, gpu_copy_time)
        # Total segments after first = num_segments - 1
        steady_time = (num_segments - 1) * steady_per_segment
        # Final GPU copy for last segment
        drain_time = gpu_copy_time

        return fill_time + steady_time + drain_time

    def batch_promote_latency(
        self, entries_per_drive: dict[int, int], size_bytes: int
    ) -> float:
        """Compute batch cold promotion latency across multiple drives.

        entries_per_drive: {drive_idx: num_cold_entries}
        Returns total wall-clock time (bounded by slowest drive).

        Models per-drive parallelism with MAX_QUEUES_PER_DRIVE threads,
        each with reduced pipeline depth.
        """
        if not entries_per_drive:
            return 0.0

        max_queues = self.config.max_queues_per_drive
        per_thread_depth = self.config.max_queue_depth // max_queues

        per_drive_time: list[float] = []
        for drive_idx, num_entries in entries_per_drive.items():
            # Split entries across threads for this drive
            entries_per_thread = (num_entries + max_queues - 1) // max_queues
            # Each thread promotes its entries sequentially
            thread_time = entries_per_thread * self._single_entry_with_depth(
                size_bytes, per_thread_depth
            )
            per_drive_time.append(thread_time)

        # Drives operate in parallel; total = slowest drive
        return max(per_drive_time) if per_drive_time else 0.0

    def _single_entry_with_depth(self, size_bytes: int, queue_depth: int) -> float:
        """Single entry promotion with a specific queue depth."""
        num_segments = max(1, (size_bytes + self.config.mdts_bytes - 1) // self.config.mdts_bytes)
        segment_size = min(size_bytes, self.config.mdts_bytes)

        nvme_read_time = self.config.nvme_read_latency_us
        gpu_copy_time = self.gpu.h2d_latency(segment_size)

        if num_segments == 1:
            return nvme_read_time + gpu_copy_time

        fill_time = nvme_read_time
        steady_per_segment = max(nvme_read_time, gpu_copy_time)
        steady_time = (num_segments - 1) * steady_per_segment
        drain_time = gpu_copy_time

        return fill_time + steady_time + drain_time

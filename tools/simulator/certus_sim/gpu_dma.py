"""GPU DMA timing model.

Models latency for GPU memory operations: H2D/D2H copies and IPC handle
open/close. Latency scales with transfer size using a bandwidth model.
"""

from __future__ import annotations

from certus_sim.config import SimConfig


class GpuDmaModel:
    """Computes GPU DMA transfer times."""

    # Approximate bandwidth limits (bytes/us)
    GPU_H2D_BANDWIDTH = 12.0e3  # ~12 GB/s PCIe Gen4 x16
    GPU_D2H_BANDWIDTH = 12.0e3

    def __init__(self, config: SimConfig):
        self.config = config

    def h2d_latency(self, size_bytes: int) -> float:
        """Host-to-device DMA latency in microseconds."""
        bandwidth_time = size_bytes / self.GPU_H2D_BANDWIDTH
        return max(self.config.gpu_h2d_latency_us, bandwidth_time)

    def d2h_latency(self, size_bytes: int) -> float:
        """Device-to-host DMA latency in microseconds."""
        bandwidth_time = size_bytes / self.GPU_D2H_BANDWIDTH
        return max(self.config.gpu_d2h_latency_us, bandwidth_time)

    def ipc_open_latency(self) -> float:
        return self.config.ipc_open_latency_us

    def ipc_close_latency(self) -> float:
        return self.config.ipc_close_latency_us

# SPDX-License-Identifier: Apache-2.0
"""CertusOffloadingSpec — OffloadingSpec for tiered DRAM + NVMe storage.

Plugs into vLLM's OffloadingConnector via kv_connector_extra_config:
{
    "spec_name": "CertusOffloadingSpec",
    "spec_module_path": "certus_connector.spec",
    "nvme_device": "/dev/nvme0n1",
    "nvme_namespace_id": 1,
    "slab_size_bytes": 131072,
    "dram_cache_bytes": 8589934592,
    "promotion_touch_threshold": 3,
    "promotion_window_seconds": 10.0,
    "demotion_idle_seconds": 30.0,
    "spdk_json_config": "/etc/certus/spdk.json",
    "io_queue_depth": 128
}
"""

from __future__ import annotations

from collections.abc import Iterator

from vllm.config import VllmConfig
from vllm.v1.kv_cache_interface import KVCacheConfig
from vllm.v1.kv_offload.abstract import LoadStoreSpec, OffloadingManager
from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
from vllm.v1.kv_offload.spec import OffloadingSpec
from vllm.v1.kv_offload.worker.worker import OffloadingHandler

from certus_connector.handler import (
    CertusTransferEngine,
    CertusToGpuHandler,
    GpuToCertusHandler,
    MockCertusTransferEngine,
)
from certus_connector.manager import CertusOffloadingManager, TieringConfig
from certus_connector.mediums import CertusLoadStoreSpec
from certus_connector.native_manager import NativeCertusOffloadingManager


def _try_native_engine(extra_config: dict):
    """Try to create a certus_native.CertusEngine. Returns None if unavailable."""
    try:
        import certus_native
        return certus_native.CertusEngine({
            "data_pci_addrs": extra_config.get("data_pci_addrs", []),
            "metadata_pci_addr": extra_config.get("metadata_pci_addr", ""),
            "gpu_block_size": int(extra_config.get("slab_size_bytes", 131072)),
        })
    except (ImportError, RuntimeError):
        return None


def _load_engine(extra_config: dict) -> CertusTransferEngine:
    """Load the mock transfer engine (for handler-based I/O path)."""
    use_mock = extra_config.get("use_mock_engine", False)
    if use_mock:
        return MockCertusTransferEngine()
    try:
        from certus_native import CertusTransferEngine as NativeEngine
        return NativeEngine(
            nvme_device=extra_config.get("nvme_device", "/dev/nvme0n1"),
            namespace_id=int(extra_config.get("nvme_namespace_id", 1)),
            spdk_config=extra_config.get("spdk_json_config", ""),
            io_queue_depth=int(extra_config.get("io_queue_depth", 128)),
            dram_bytes=int(extra_config.get("dram_cache_bytes", 0)),
        )
    except ImportError:
        return MockCertusTransferEngine()


class CertusOffloadingSpec(OffloadingSpec):
    """OffloadingSpec for tiered DRAM + raw NVMe storage via SPDK.

    Blocks are content-addressable (hash-indexed). Storage uses a slab
    allocator on raw NVMe (no filesystem). Hot blocks are cached in
    pinned DRAM with policy-driven promotion/demotion.
    """

    def __init__(self, vllm_config: VllmConfig, kv_cache_config: KVCacheConfig):
        super().__init__(vllm_config, kv_cache_config)

        assert len(self.gpu_block_size) == 1, (
            "CertusOffloadingSpec requires exactly one KV cache group"
        )
        gpu_bs = self.gpu_block_size[0]
        self._offloaded_block_size = gpu_bs * self.block_size_factor

        # Compute slab/slot capacities from byte budgets
        slab_size_bytes = int(self.extra_config.get("slab_size_bytes", 131072))
        dram_cache_bytes = int(self.extra_config.get("dram_cache_bytes", 0))
        max_dram_slots = dram_cache_bytes // slab_size_bytes if slab_size_bytes > 0 else 0

        # NVMe capacity: 0 = unlimited (for testing)
        max_nvme_slabs = int(self.extra_config.get("max_nvme_slabs", 0))

        self._tiering_config = TieringConfig(
            max_nvme_slabs=max_nvme_slabs,
            max_dram_slots=max_dram_slots,
            promotion_touch_threshold=int(
                self.extra_config.get("promotion_touch_threshold", 3)
            ),
            promotion_window_seconds=float(
                self.extra_config.get("promotion_window_seconds", 10.0)
            ),
            demotion_idle_seconds=float(
                self.extra_config.get("demotion_idle_seconds", 30.0)
            ),
        )

        self._slab_size_bytes = slab_size_bytes
        self._engine: CertusTransferEngine | None = None
        self._manager: CertusOffloadingManager | None = None
        self._gpu_to_certus: GpuToCertusHandler | None = None
        self._certus_to_gpu: CertusToGpuHandler | None = None

    def _get_engine(self) -> CertusTransferEngine:
        if self._engine is None:
            self._engine = _load_engine(self.extra_config)
        return self._engine

    def get_manager(self) -> OffloadingManager:
        if self._manager is None:
            use_native = self.extra_config.get("use_native", True)
            if use_native:
                native_engine = _try_native_engine(self.extra_config)
                if native_engine is not None:
                    self._native_engine = native_engine
                    self._manager = NativeCertusOffloadingManager(native_engine)
                    return self._manager

            kv_events_config = self.vllm_config.kv_events_config
            enable_events = (
                kv_events_config is not None
                and kv_events_config.enable_kv_cache_events
            )
            self._manager = CertusOffloadingManager(
                config=self._tiering_config,
                enable_events=enable_events,
            )
        return self._manager

    def get_handlers(
        self,
        kv_caches,
        attn_backends=None,
    ) -> Iterator[tuple[type[LoadStoreSpec], type[LoadStoreSpec], OffloadingHandler]]:
        engine = self._get_engine()
        if self._gpu_to_certus is None:
            self._gpu_to_certus = GpuToCertusHandler(
                engine=engine,
                block_size_bytes=self._slab_size_bytes,
            )
        if self._certus_to_gpu is None:
            self._certus_to_gpu = CertusToGpuHandler(
                engine=engine,
                block_size_bytes=self._slab_size_bytes,
            )
        yield GPULoadStoreSpec, CertusLoadStoreSpec, self._gpu_to_certus
        yield CertusLoadStoreSpec, GPULoadStoreSpec, self._certus_to_gpu

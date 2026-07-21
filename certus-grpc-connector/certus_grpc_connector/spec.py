# SPDX-License-Identifier: Apache-2.0
"""CertusGrpcOffloadingSpec — vLLM OffloadingSpec talking to a remote certus-server.

Plugs into vLLM's OffloadingConnector via kv_connector_extra_config:
{
    "spec_name": "CertusGrpcOffloadingSpec",
    "spec_module_path": "certus_grpc_connector.spec",
    "server": "localhost:50051",
    "slab_size_bytes": 131072
}

Unlike the in-process ``certus-connector``, no SPDK/CUDA engine is embedded: the
server owns the hardware and this process only opens a gRPC channel and shares
CUDA IPC handles for its KV-cache blocks.
"""

from __future__ import annotations

from collections.abc import Iterator

from vllm.config import VllmConfig
from vllm.v1.kv_cache_interface import KVCacheConfig
from vllm.v1.kv_offload.abstract import LoadStoreSpec, OffloadingManager
from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
from vllm.v1.kv_offload.spec import OffloadingSpec
from vllm.v1.kv_offload.worker.worker import OffloadingHandler

from .client import make_stub
from .gpu import current_device, ipc_for_tensor
from .handler import CertusToGpuHandler, GpuToCertusHandler
from .manager import GrpcCertusOffloadingManager
from .mediums import CertusLoadStoreSpec

# Process-level singletons: one channel/stub and one background executor per
# worker process, shared across manager + handlers.
_CHANNEL_SINGLETON = None
_STUB_SINGLETON = None


def _get_or_create_stub(server: str):
    global _CHANNEL_SINGLETON, _STUB_SINGLETON
    if _STUB_SINGLETON is None:
        _CHANNEL_SINGLETON, _STUB_SINGLETON = make_stub(server)
    return _STUB_SINGLETON


class CertusGrpcOffloadingSpec(OffloadingSpec):
    """OffloadingSpec backed by a remote certus-server over gRPC."""

    def __init__(self, vllm_config: VllmConfig, kv_cache_config: KVCacheConfig):
        super().__init__(vllm_config, kv_cache_config)

        assert len(self.gpu_block_size) == 1, (
            "CertusGrpcOffloadingSpec requires exactly one KV cache group"
        )
        gpu_bs = self.gpu_block_size[0]
        self._offloaded_block_size = gpu_bs * self.block_size_factor
        # slab_size_bytes is only a *fallback* for the Reserve allocation size
        # used before the KV-cache tensor stride is known (get_manager can run
        # before get_handlers). It must NOT be used as the per-block transfer
        # size: the real per-block byte size is the tensor's stride(0), and the
        # server addresses blocks at block_id * stride. Copying more than stride
        # per block overruns the IPC allocation (H2D "invalid argument") and
        # corrupts neighbouring blocks. See _extract_gpu_ptrs / get_handlers.
        self._slab_size_bytes = int(self.extra_config.get("slab_size_bytes", 131072))
        self._block_bytes: int | None = None
        self._server = str(self.extra_config.get("server", "localhost:50051"))

        self._stub = None
        self._manager: GrpcCertusOffloadingManager | None = None
        self._gpu_to_certus: GpuToCertusHandler | None = None
        self._certus_to_gpu: CertusToGpuHandler | None = None

    def _get_stub(self):
        if self._stub is None:
            self._stub = _get_or_create_stub(self._server)
        return self._stub

    def get_manager(self) -> OffloadingManager:
        if self._manager is None:
            # Prefer the true per-block size (tensor stride) if handlers have
            # already resolved it; otherwise fall back to the configured slab
            # size, which is a safe upper bound (Reserve size only needs to be
            # >= the copy size). get_handlers() corrects the manager once the
            # stride is known.
            size = self._block_bytes if self._block_bytes is not None else self._slab_size_bytes
            self._manager = GrpcCertusOffloadingManager(
                self._get_stub(), block_size_bytes=size
            )
        return self._manager

    def get_handlers(
        self,
        kv_caches,
        attn_backends=None,
    ) -> Iterator[tuple[type[LoadStoreSpec], type[LoadStoreSpec], OffloadingHandler]]:
        from concurrent.futures import ThreadPoolExecutor

        stub = self._get_stub()
        if self._gpu_to_certus is None:
            data_ptr, stride = self._extract_gpu_ptrs(kv_caches)
            kv = ipc_for_tensor(data_ptr, stride, current_device())
            # The per-block transfer size is exactly the tensor's per-block
            # stride: the server addresses each block at block_id * stride, and
            # the handlers map one block_id per key (1:1). Copying `stride` bytes
            # per block is the only size that stays within each block's extent
            # in the IPC allocation. Do NOT use slab_size_bytes here.
            block_bytes = stride
            self._block_bytes = block_bytes
            if self._manager is not None:
                # Manager was created before the stride was known; correct its
                # Reserve size so DRAM/SSD slots match the actual block size.
                self._manager.set_block_size_bytes(block_bytes)
            if block_bytes > self._slab_size_bytes:
                print(
                    f"[certus-grpc] WARNING: per-block size {block_bytes} exceeds "
                    f"slab_size_bytes {self._slab_size_bytes}; using {block_bytes}",
                    flush=True,
                )
            print(
                f"[certus-grpc] KV base=0x{data_ptr:x} stride={stride} "
                f"block_bytes={block_bytes} slab_size_bytes={self._slab_size_bytes} "
                f"device={kv.gpu_device_id} base_delta={kv.base_delta}",
                flush=True,
            )
            executor = ThreadPoolExecutor(max_workers=4, thread_name_prefix="certus-grpc")
            self._gpu_to_certus = GpuToCertusHandler(
                stub, kv, block_bytes, executor
            )
            self._certus_to_gpu = CertusToGpuHandler(
                stub, kv, block_bytes, executor
            )
        yield GPULoadStoreSpec, CertusLoadStoreSpec, self._gpu_to_certus
        yield CertusLoadStoreSpec, GPULoadStoreSpec, self._certus_to_gpu

    @staticmethod
    def _extract_gpu_ptrs(kv_caches) -> tuple[int, int]:
        """Extract GPU base pointer and per-block stride (bytes) from the first tensor."""
        tensor = kv_caches.tensors[0].tensor
        stride_bytes = tensor.stride(0) * tensor.element_size()
        return tensor.data_ptr(), stride_bytes

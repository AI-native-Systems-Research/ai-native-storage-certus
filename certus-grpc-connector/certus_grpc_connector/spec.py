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

        # Per-block Reserve size. CRITICAL: the manager (which issues Reserve) and
        # the handlers (which issue the GPU->DRAM copy) live in SEPARATE spec
        # instances — vLLM instantiates the connector twice, once per role
        # (scheduler builds the manager, worker builds the handlers). The
        # scheduler-side instance NEVER calls get_handlers, so it can't learn the
        # KV tensor stride; if the manager Reserves a slot smaller than the copy
        # size, the server's D2H copy fails its bounds check ("size (X) exceeds
        # destination buffer length (Y)") for EVERY block and nothing is ever
        # cached — silently, because the store handler must report success.
        #
        # So derive the true per-block byte size from the KV-cache config here
        # (available to BOTH roles at construction), not from the tensor. It is
        # the offloaded page size = per-GPU-block page_size_bytes * block_size_factor.
        # slab_size_bytes is a last-resort fallback only.
        self._slab_size_bytes = int(self.extra_config.get("slab_size_bytes", 131072))
        self._block_bytes: int | None = self._block_bytes_from_config(kv_cache_config)
        self._server = str(self.extra_config.get("server", "localhost:50051"))

        self._stub = None
        self._manager: GrpcCertusOffloadingManager | None = None
        self._gpu_to_certus: GpuToCertusHandler | None = None
        self._certus_to_gpu: CertusToGpuHandler | None = None

    def _block_bytes_from_config(self, kv_cache_config: KVCacheConfig) -> int | None:
        """True offloaded per-block size in bytes, derived from the KV-cache
        config (not the GPU tensor, which only the worker role can see).

        = per-GPU-block ``page_size_bytes`` * ``block_size_factor``. Returns
        None if the config can't be read, in which case get_manager falls back
        to slab_size_bytes."""
        try:
            groups = kv_cache_config.kv_cache_groups
            if len(groups) != 1:
                return None
            # page_size_bytes is PER LAYER (2 * block_size * kv_heads * head_dim
            # * dtype). This connector offloads one GPU block across ALL layers
            # in the group per key — the KV tensor's stride(0) spans every layer
            # — so the per-block Reserve size is page_size_bytes * num_layers.
            # (Confirmed: granite 65536/layer * 40 layers = 2621440 = stride(0).)
            num_layers = len(groups[0].layer_names)
            page = int(groups[0].kv_cache_spec.page_size_bytes)
            block_bytes = page * num_layers * self.block_size_factor
            print(
                f"[certus-grpc] per-block Reserve size from KV-cache config: "
                f"page_size_bytes={page} * num_layers={num_layers} * "
                f"block_size_factor={self.block_size_factor} = {block_bytes} bytes",
                flush=True,
            )
            return block_bytes
        except Exception as e:  # noqa: BLE001 - fall back to slab_size_bytes
            print(
                f"[certus-grpc] WARNING: could not derive per-block size from "
                f"KV-cache config ({e}); falling back to slab_size_bytes "
                f"{self._slab_size_bytes}. If it is smaller than the real block, "
                f"stores will fail their D2H bounds check.",
                flush=True,
            )
            return None

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
            # in the IPC allocation. This is the authoritative COPY size.
            block_bytes = stride
            # Cross-check against the config-derived Reserve size the manager
            # uses (a DIFFERENT spec instance in the scheduler role). If these
            # disagree, the server will Reserve a slot that doesn't match the
            # copy and every store fails its D2H bounds check — the exact
            # silent-offload bug this connector hit on the granite model swap.
            if self._block_bytes is not None and self._block_bytes != block_bytes:
                print(
                    f"[certus-grpc] WARNING: tensor stride {block_bytes} != "
                    f"config-derived Reserve size {self._block_bytes}. Reserve "
                    f"slots will not match the copy size and stores may fail "
                    f"their D2H bounds check. Using {block_bytes} for the copy.",
                    flush=True,
                )
            self._block_bytes = block_bytes
            if self._manager is not None:
                # If a manager exists in this instance, keep its Reserve size in
                # sync with the copy size (belt-and-suspenders; the scheduler-role
                # instance already sized itself from the config in __init__).
                self._manager.set_block_size_bytes(block_bytes)
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

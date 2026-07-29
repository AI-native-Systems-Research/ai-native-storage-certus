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

from .compat import (
    CAPS,
    GPULoadStoreSpec,
    LoadStoreSpec,
    OffloadingManager,
    OffloadingSpec,
    block_bytes_from_config,
    block_bytes_from_offloading_config,
    extract_gpu_ptrs,
)
from .client import make_stub
from .gpu import current_device, ipc_for_tensor
from .handler import worker_class
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

    def __init__(self, *args):
        # The base ctor signature changed with the 0.26 API rewrite:
        #   ≤0.24: __init__(vllm_config, kv_cache_config) — base exposes
        #          ``gpu_block_size`` / ``block_size_factor``.
        #   0.26 : __init__(config: OffloadingConfig) — base exposes ``config`` /
        #          ``extra_config``; per-block bytes come from
        #          ``config.worker_kv_bytes_per_block`` directly.
        # Branch on the named capability so the two eras share one class.
        #
        # Per-block Reserve size. CRITICAL: the manager (which issues Reserve) and
        # the worker (which issues the GPU->DRAM copy) live in SEPARATE spec
        # instances — vLLM instantiates the connector twice, once per role
        # (scheduler builds the manager, worker builds the worker/handlers). The
        # scheduler-side instance NEVER calls get_worker/get_handlers, so it can't
        # learn the KV tensor stride; if the manager Reserves a slot smaller than
        # the copy size, the server's D2H copy fails its bounds check ("size (X)
        # exceeds destination buffer length (Y)") for EVERY block and nothing is
        # ever cached — silently, because the store path must report success. So
        # derive the true per-block byte size at construction (available to BOTH
        # roles), not from the tensor. slab_size_bytes is a last-resort fallback.
        if CAPS.spec_config_object:
            (config,) = args
            super().__init__(config)
            self._slab_size_bytes = int(self.extra_config.get("slab_size_bytes", 131072))
            # 0.26 hands the per-block bytes to us directly.
            self._block_bytes: int | None = block_bytes_from_offloading_config(config)
        else:
            vllm_config, kv_cache_config = args
            super().__init__(vllm_config, kv_cache_config)
            assert len(self.gpu_block_size) == 1, (
                "CertusGrpcOffloadingSpec requires exactly one KV cache group"
            )
            gpu_bs = self.gpu_block_size[0]
            self._offloaded_block_size = gpu_bs * self.block_size_factor
            self._slab_size_bytes = int(self.extra_config.get("slab_size_bytes", 131072))
            # ≤0.24: reconstruct the offloaded page size from the KV-cache config
            # (per-GPU-block page_size_bytes * num_layers * block_size_factor).
            self._block_bytes = block_bytes_from_config(
                kv_cache_config, self.block_size_factor
            )

        self._server = str(self.extra_config.get("server", "localhost:50051"))

        self._stub = None
        self._manager: GrpcCertusOffloadingManager | None = None
        self._worker = None

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

    def _ensure_worker(self, kv_caches):
        """Resolve the GPU IPC handle from the KV-cache handoff and build the
        single ``CertusGrpcWorker`` (one instance serves both directions). Shared
        by ``get_worker`` (0.26) and ``get_handlers`` (≤0.24)."""
        from concurrent.futures import ThreadPoolExecutor

        if self._worker is not None:
            return self._worker

        stub = self._get_stub()
        data_ptr, stride = extract_gpu_ptrs(kv_caches)
        kv = ipc_for_tensor(data_ptr, stride, current_device())
        # The per-block transfer size is exactly the tensor's per-block stride:
        # the server addresses each block at block_id * stride, and the worker
        # maps one block_id per key (1:1). Copying `stride` bytes per block is the
        # only size that stays within each block's extent in the IPC allocation.
        # This is the authoritative COPY size.
        block_bytes = stride
        # Cross-check against the config-derived Reserve size the manager uses (a
        # DIFFERENT spec instance in the scheduler role). If these disagree, the
        # server will Reserve a slot that doesn't match the copy and every store
        # fails its D2H bounds check — the exact silent-offload bug this connector
        # hit on the granite model swap. On 0.26 the config value is
        # ``worker_kv_bytes_per_block``, which should equal the single canonical
        # tensor's stride; a mismatch here means the canonical layout is not the
        # one contiguous block the connector assumes.
        if self._block_bytes is not None and self._block_bytes != block_bytes:
            print(
                f"[certus-grpc] WARNING: tensor stride {block_bytes} != "
                f"config-derived Reserve size {self._block_bytes}. Reserve slots "
                f"will not match the copy size and stores may fail their D2H "
                f"bounds check. Using {block_bytes} for the copy.",
                flush=True,
            )
        self._block_bytes = block_bytes
        if self._manager is not None:
            # If a manager exists in this instance, keep its Reserve size in sync
            # with the copy size (belt-and-suspenders; the scheduler-role instance
            # already sized itself from the config in __init__).
            self._manager.set_block_size_bytes(block_bytes)
        print(
            f"[certus-grpc] KV base=0x{data_ptr:x} stride={stride} "
            f"block_bytes={block_bytes} slab_size_bytes={self._slab_size_bytes} "
            f"device={kv.gpu_device_id} base_delta={kv.base_delta}",
            flush=True,
        )
        executor = ThreadPoolExecutor(max_workers=4, thread_name_prefix="certus-grpc")
        self._worker = worker_class()(stub, kv, block_bytes, executor)
        return self._worker

    def get_worker(self, kv_caches):
        """0.26+: return the single ``OffloadingWorker`` for this spec.

        ``kv_caches`` is a ``CanonicalKVCaches``; ``extract_gpu_ptrs`` (inside
        ``_ensure_worker``) logs its tensor layout and refuses a split (multi-
        tensor) layout the one-IPC-region-per-key model can't represent, so a
        mismatch is caught before any store rather than silently truncated."""
        return self._ensure_worker(kv_caches)

    def get_handlers(
        self,
        kv_caches,
        attn_backends=None,
    ) -> Iterator[tuple[type[LoadStoreSpec], type[LoadStoreSpec], object]]:
        """≤0.24: yield the SAME worker instance for both medium pairs. Its
        ``transfer_async`` routes to the store or load body by the source spec's
        type, so one instance serves both directions."""
        worker = self._ensure_worker(kv_caches)
        yield GPULoadStoreSpec, CertusLoadStoreSpec, worker
        yield CertusLoadStoreSpec, GPULoadStoreSpec, worker

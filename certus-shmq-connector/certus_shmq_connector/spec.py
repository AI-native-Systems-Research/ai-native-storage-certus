# SPDX-License-Identifier: Apache-2.0
"""CertusShmqOffloadingSpec — vLLM OffloadingSpec talking to a remote
certus-server over a shared-memory ring (drop-in for the gRPC spec).

Plugs into vLLM's OffloadingConnector via kv_connector_extra_config:
{
    "spec_name": "CertusShmqOffloadingSpec",
    "spec_module_path": "certus_shmq_connector.spec",
    "shm_path": "/dev/shm/certus-shmq",
    "slab_size_bytes": 131072
}

The process-level singleton is a ``Ring`` attached to the ``/dev/shm`` mailbox
(keyed by ``shm_path``). The server owns the hardware; this process only mmaps
the ring and shares CUDA IPC handles for its KV-cache blocks.
"""

from __future__ import annotations

import os
import threading
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
from .gpu import current_device, ipc_for_tensor
from .handler import worker_class
from .manager import ShmqCertusOffloadingManager
from .mediums import CertusLoadStoreSpec
from .ring import Ring

# Process-level singleton: one attached Ring per worker process, keyed by shm
# path, shared across manager + handlers. Attaching spins on the header READY
# flag, so the first spec instance blocks until the server is up.
_RING_LOCK = threading.Lock()
_RING_SINGLETONS: dict[str, Ring] = {}


def _get_or_create_ring(shm_path: str, claim_slot: int, claim_slots: int) -> Ring:
    with _RING_LOCK:
        ring = _RING_SINGLETONS.get(shm_path)
        if ring is None:
            ring = Ring(shm_path, claim_slot=claim_slot, claim_slots=claim_slots)
            _RING_SINGLETONS[shm_path] = ring
        return ring


def _resolve_world_size() -> int:
    """TP world size — the uniform source across all connector processes.

    The benchmark exports ``TENSOR_PARALLEL_SIZE`` and every vLLM subprocess
    (engine-core + each TP worker) inherits it, so it is readable in all three
    processes before vLLM's own distributed state exists. Default 1 = single-GPU
    baseline (no namespacing, one channel slot)."""
    try:
        return max(1, int(os.environ.get("TENSOR_PARALLEL_SIZE", "1")))
    except ValueError:
        return 1


def _resolve_tp_rank() -> int:
    """This worker's TP shard rank. KV heads are sharded by exactly this rank, so
    it MUST be vLLM's real tensor-parallel rank — used to namespace store keys.

    Primary: ``vllm.distributed`` TP rank (authoritative). Fallbacks (only if the
    distributed group isn't initialised yet): the CUDA device index — equal to
    the TP rank under the standard single-node one-GPU-per-rank mapping — then the
    ``LOCAL_RANK``/``RANK`` env, then 0."""
    try:
        from vllm.distributed import get_tensor_model_parallel_rank

        return int(get_tensor_model_parallel_rank())
    except Exception:  # noqa: BLE001 - group not init'd yet / import path moved
        pass
    try:
        from vllm.distributed import parallel_state

        return int(parallel_state.get_tensor_model_parallel_rank())
    except Exception:  # noqa: BLE001
        pass
    try:
        return int(current_device())
    except Exception:  # noqa: BLE001
        pass
    for _var in ("LOCAL_RANK", "RANK"):
        _v = os.environ.get(_var)
        if _v is not None:
            try:
                return int(_v)
            except ValueError:
                pass
    return 0


def _resolve_dp_size() -> int:
    """Data-parallel replica count sharing one certus-server mailbox.

    Each DP replica is an independent vLLM engine on its own GPU, all driving a
    single shared server. The benchmark exports ``DP_SIZE`` to every replica's
    processes. Default 1 = single-replica baseline (no DP partitioning)."""
    try:
        return max(1, int(os.environ.get("DP_SIZE", "1")))
    except ValueError:
        return 1


def _resolve_dp_rank() -> int:
    """This replica's data-parallel index in ``[0, DP_SIZE)``.

    Set per-container by the harness (``DP_RANK``). Used only to offset this
    replica's channel-claim partition so two replicas sharing one mailbox scan
    disjoint channel slices. Default 0."""
    try:
        return max(0, int(os.environ.get("DP_RANK", "0")))
    except ValueError:
        return 0


class CertusShmqOffloadingSpec(OffloadingSpec):
    """OffloadingSpec backed by a remote certus-server over shared memory."""

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
                "CertusShmqOffloadingSpec requires exactly one KV cache group"
            )
            gpu_bs = self.gpu_block_size[0]
            self._offloaded_block_size = gpu_bs * self.block_size_factor
            self._slab_size_bytes = int(self.extra_config.get("slab_size_bytes", 131072))
            # ≤0.24: reconstruct the offloaded page size from the KV-cache config
            # (per-GPU-block page_size_bytes * num_layers * block_size_factor).
            self._block_bytes = block_bytes_from_config(
                kv_cache_config, self.block_size_factor
            )

        self._shm_path = str(self.extra_config.get("shm_path", "/dev/shm/certus-shmq"))

        # TP topology. world_size is uniform across all processes (env); the
        # per-worker rank is resolved lazily in _ensure_worker (only the worker
        # role needs its own rank — the scheduler-role manager operates over ALL
        # ranks). channel-claim slots: W==1 → 1 (single-process baseline); W>1 →
        # W+1 (scheduler slot 0 + one slot per worker rank), because TP>1 uses
        # MultiprocExecutor and the engine-core + W workers share the mailbox.
        self._world_size = _resolve_world_size()
        # Per-replica channel slots: TP=1 → 1 (UniProcExecutor); TP>1 → W+1
        # (scheduler slot 0 + one per worker rank, MultiprocExecutor).
        self._per_replica_slots = 1 if self._world_size <= 1 else self._world_size + 1
        # Data-parallel replicas share the one mailbox, so the global slot space
        # is per_replica_slots * DP_SIZE and this replica's slots are offset by
        # dp_rank * per_replica_slots — each DP replica claims a disjoint channel
        # slice, on top of the TP slot split within a replica. DP_SIZE=1 → offset
        # 0, exact single-replica baseline.
        self._dp_size = _resolve_dp_size()
        self._dp_rank = _resolve_dp_rank()
        self._dp_slot_base = self._dp_rank * self._per_replica_slots
        self._claim_slots = self._per_replica_slots * self._dp_size

        self._ring = None
        self._manager: ShmqCertusOffloadingManager | None = None
        self._worker = None

    def _get_ring(self, claim_slot: int = 0):
        if self._ring is None:
            self._ring = _get_or_create_ring(
                self._shm_path, claim_slot, self._claim_slots
            )
        return self._ring

    def get_manager(self) -> OffloadingManager:
        if self._manager is None:
            # Prefer the true per-block size (tensor stride) if handlers have
            # already resolved it; otherwise fall back to the configured slab
            # size, which is a safe upper bound (Reserve size only needs to be
            # >= the copy size). get_handlers() corrects the manager once the
            # stride is known.
            size = self._block_bytes if self._block_bytes is not None else self._slab_size_bytes
            # Scheduler role → this replica's base channel slot. The manager
            # mirrors what all W workers physically store, so it operates over
            # every per-rank key.
            self._manager = ShmqCertusOffloadingManager(
                self._get_ring(claim_slot=self._dp_slot_base),
                block_size_bytes=size,
                world_size=self._world_size,
            )
        return self._manager

    def _ensure_worker(self, kv_caches):
        """Resolve the GPU IPC handle from the KV-cache handoff and build the
        single ``CertusShmqWorker`` (one instance serves both directions). Shared
        by ``get_worker`` (0.26) and ``get_handlers`` (≤0.24)."""
        from concurrent.futures import ThreadPoolExecutor

        if self._worker is not None:
            return self._worker

        # Worker role → resolve this process's TP shard rank and claim channel
        # slot rank+1 (slot 0 belongs to the scheduler/manager process). At W==1
        # slots==1 so the slot is ignored (full-range baseline).
        rank = _resolve_tp_rank() if self._world_size > 1 else 0
        local_slot = (rank + 1) if self._world_size > 1 else 0
        claim_slot = self._dp_slot_base + local_slot
        ring = self._get_ring(claim_slot=claim_slot)
        # 0.23+ splits a block into N per-layer tensors, each a separate GPU
        # allocation; 0.20/0.22 present one coalesced tensor (N==1). We open one
        # IPC handle per region and store/load the block as N colocated regions in
        # one slot — see docs/multi-region-kv-offload.md. extract_gpu_ptrs returns
        # a list of (ptr, stride) either way, so there is no version branch here.
        regions = extract_gpu_ptrs(kv_caches)
        kv_regions = [
            ipc_for_tensor(ptr, stride, current_device())
            for ptr, stride in regions
        ]
        # The authoritative per-block COPY size is the SUM of the per-region
        # strides: the server lays the N regions out contiguously in one slot of
        # this many bytes (single-tensor case: the one stride == full block). Each
        # region copies its own stride bytes; the worker maps one block_id per key
        # (1:1), so total bytes per block is Σ stride.
        block_bytes = sum(stride for _, stride in regions)
        # Cross-check against the config-derived Reserve size the manager uses (a
        # DIFFERENT spec instance in the scheduler role). If these disagree, the
        # server will Reserve a slot that doesn't match the copy and every store
        # fails its D2H bounds check — the exact silent-offload bug this connector
        # hit on the granite model swap.
        if self._block_bytes is not None and self._block_bytes != block_bytes:
            print(
                f"[certus-shmq] WARNING: summed region strides {block_bytes} != "
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
            f"[certus-shmq] KV {len(kv_regions)} region(s) block_bytes={block_bytes} "
            f"slab_size_bytes={self._slab_size_bytes} "
            f"device={kv_regions[0].gpu_device_id} "
            f"tp_rank={rank}/{self._world_size} "
            f"per-region strides={[r.stride_bytes for r in kv_regions]}",
            flush=True,
        )
        executor = ThreadPoolExecutor(max_workers=4, thread_name_prefix="certus-shmq")
        self._worker = worker_class()(
            ring, kv_regions, block_bytes, executor,
            rank=rank, world_size=self._world_size,
        )
        return self._worker

    def get_worker(self, kv_caches):
        """0.26+: return the single ``OffloadingWorker`` for this spec.

        ``kv_caches`` is a ``CanonicalKVCaches``; ``extract_gpu_ptrs`` (inside
        ``_ensure_worker``) returns one (ptr, stride) per layer tensor, which the
        worker stores as N colocated regions per block — a split (multi-tensor)
        layout is handled, not refused (see docs/multi-region-kv-offload.md)."""
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

# SPDX-License-Identifier: Apache-2.0
"""OffloadingManager backed by a remote certus-server over gRPC.

All index/allocation/eviction state lives in the server. This class adapts
between vLLM's Python types and the server's u64-keyed batch RPCs, per the
mapping:

    lookup        -> local _presence/_inflight sets (HIT/HIT_PENDING/MISS)
    prepare_store -> (no RPC; StoreBatch is atomic on the worker side)
    complete_store-> (no RPC; updates local presence tracking)
    prepare_load  -> Pin (eviction protection until complete_load)
    complete_load -> Unpin
    touch         -> Touch
    take_events   -> TakeEvents(max_events=0)
"""

from __future__ import annotations

import hashlib
import sys
from collections.abc import Iterable

from .compat import (
    LoadStoreSpec,
    OffloadingEvent,
    OffloadingManager,
    OffloadKey,
    PrepareStoreOutput,
)

from . import dispatcher_pb2 as pb
from .mediums import BlockLocation, CertusLoadStoreSpec


def _key_to_u64(key: OffloadKey) -> int:
    """Convert an OffloadKey (bytes) to a u64 for the server."""
    if isinstance(key, int):
        return key
    return int.from_bytes(key[:8], "big")


def _keys_to_u64s(keys: Iterable[OffloadKey]) -> list[int]:
    return [_key_to_u64(k) for k in keys]


def _session_id_to_u64(req_context) -> int:
    """Extract a session id from the request context and fold it to a u64.

    vLLM exposes per-request custom params via ``req_context.kv_transfer_params``
    (a dict populated from ``SamplingParams.extra_args["kv_transfer_params"]``).
    A caller opts in by setting ``{"session_id": <str|int>}`` there. The server
    and Certus are u64-keyed, so a string id is folded to u64 with a stable hash
    (BLAKE2b, first 8 bytes big-endian). Returns 0 (== "unset") when absent, so
    clients that don't set it stay wire-compatible.
    """
    kv_params = getattr(req_context, "kv_transfer_params", None)
    if not kv_params:
        return 0
    sid = kv_params.get("session_id")
    if sid is None:
        return 0
    if isinstance(sid, int):
        return sid & 0xFFFFFFFFFFFFFFFF
    digest = hashlib.blake2b(str(sid).encode("utf-8"), digest_size=8).digest()
    return int.from_bytes(digest, "big")


class GrpcCertusOffloadingManager(OffloadingManager):
    """Manager delegating to a remote certus-server via a DispatcherStub."""

    def __init__(self, stub, block_size_bytes: int):
        self._stub = stub
        self._block_size_bytes = int(block_size_bytes)
        self._presence: set[int] = set()
        self._inflight: set[int] = set()
        self._store_dropped_blocks = 0
        self._store_drop_log_next = 1000

    def set_block_size_bytes(self, block_size_bytes: int) -> None:
        """Update the per-block Reserve size once the true KV-cache tensor
        stride is known (the manager is constructed before get_handlers can
        resolve it). Reserve sizes are per-call, so changing this affects only
        subsequent stores."""
        self._block_size_bytes = int(block_size_bytes)

    # ── request lifecycle ──

    def on_new_request(self, req_context=None):
        """Called once when the scheduler first sees a request (vLLM 0.24+).

        Returns the default ``RequestOffloadingContext`` (BLOCK_LEVEL policy:
        offload newly-computed blocks, skip prefix hits already offloaded by a
        prior request) — matching the Check filter in ``prepare_store``. This
        method is a no-op contract on <0.24 (the base neither declares nor calls
        it); the return type is built lazily through compat because it does not
        exist before 0.24.
        """
        from .compat import new_request_offloading_context

        return new_request_offloading_context()

    # ── lookup / touch ──

    def lookup(self, key: OffloadKey, req_context=None):
        from .compat import lookup_result, lookup_result_pending

        int_key = _key_to_u64(key)
        if int_key in self._inflight:
            return lookup_result_pending()
        return lookup_result(int_key in self._presence)

    def touch(self, keys: Iterable[OffloadKey], req_context=None) -> None:
        int_keys = _keys_to_u64s(keys)
        if int_keys:
            self._stub.Touch(pb.BatchTouchRequest(keys=int_keys, promote=False))

    # ── store ──

    def prepare_store(
        self, keys: Iterable[OffloadKey], req_context=None
    ) -> PrepareStoreOutput | None:
        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)

        # Filter out keys already stored or already in-flight.
        to_store_pairs = [
            (orig, k) for orig, k in zip(keys_list, int_keys)
            if k not in self._presence and k not in self._inflight
        ]

        if not to_store_pairs:
            return PrepareStoreOutput(
                keys_to_store=[],
                store_spec=CertusLoadStoreSpec([]),
                evicted_keys=[],
            )

        # No Reserve RPC — StoreBatch on the server does reserve+DMA+commit
        # atomically. We return all candidate keys; per-key failures are
        # handled in _do_store (which calls StoreBatch).
        stored_orig = [orig for orig, _ in to_store_pairs]
        stored_ints = [k for _, k in to_store_pairs]
        self._inflight.update(stored_ints)
        locations = [BlockLocation(key=k) for k in stored_ints]
        return PrepareStoreOutput(
            keys_to_store=stored_orig,
            store_spec=CertusLoadStoreSpec(locations),
            evicted_keys=[],
        )

    def _note_store_drops(self, dropped: int) -> None:
        """Account for blocks skipped due to a saturated memory tier and emit a
        throttled summary (never per-request, to avoid a warning storm)."""
        self._store_dropped_blocks += dropped
        if self._store_dropped_blocks >= self._store_drop_log_next:
            print(
                f"[certus-grpc] memory tier saturated: skipped offloading "
                f"{self._store_dropped_blocks} blocks so far (best-effort store; "
                f"blocks stay in GPU). Consider a larger --memory-tier-size or "
                f"lower concurrency.",
                file=sys.stderr,
                flush=True,
            )
            # Back off the log cadence so a persistently-full tier stays quiet.
            self._store_drop_log_next = self._store_dropped_blocks * 2

    def complete_store(
        self, keys: Iterable[OffloadKey], req_context=None, success: bool = True
    ) -> None:
        int_keys = _keys_to_u64s(keys)
        if not int_keys:
            return
        self._inflight.difference_update(int_keys)
        if success:
            self._presence.update(int_keys)

    # ── load ──

    def prepare_load(self, keys: Iterable[OffloadKey], req_context=None) -> LoadStoreSpec:
        int_keys = _keys_to_u64s(keys)
        # Pin: protect from eviction until complete_load. If a key was
        # evicted between lookup() and now, Pin fails for that key —
        # invalidate _presence so the next lookup returns MISS.
        resp = self._stub.Pin(pb.BatchPinRequest(keys=int_keys, promote=False))
        for r in resp.results:
            if not r.success:
                self._presence.discard(r.key)
                self._inflight.discard(r.key)
        return CertusLoadStoreSpec([BlockLocation(key=k) for k in int_keys])

    def complete_load(self, keys: Iterable[OffloadKey], req_context=None) -> None:
        int_keys = _keys_to_u64s(keys)
        if int_keys:
            self._stub.Unpin(pb.BatchUnpinRequest(keys=int_keys))

    # ── events / shutdown ──

    def take_events(self) -> Iterable[OffloadingEvent]:
        resp = self._stub.TakeEvents(pb.TakeEventsRequest(max_events=0))
        if resp.dropped_count:
            print(
                f"[certus-grpc] WARNING: {resp.dropped_count} eviction events "
                "dropped by server (event view is lossy)",
                file=sys.stderr,
                flush=True,
            )
            # Lost events mean our caches may be stale. Clear both so
            # lookup() returns MISS for anything we can't verify.
            self._presence.clear()
            self._inflight.clear()
        removed = []
        for e in resp.events:
            if e.reason == pb.EVICTION_REASON_REMOVED:
                self._presence.discard(e.key)
                self._inflight.discard(e.key)
                removed.append(e.key.to_bytes(8, "big"))
        if removed:
            yield OffloadingEvent(
                keys=removed,
                medium=CertusLoadStoreSpec.medium(),
                removed=True,
            )

    def shutdown(self) -> None:
        # Channel is owned by the spec singleton; nothing per-manager to close.
        pass

# SPDX-License-Identifier: Apache-2.0
"""OffloadingManager backed by a remote certus-server over gRPC.

All index/allocation/eviction state lives in the server. This class adapts
between vLLM's Python types and the server's u64-keyed batch RPCs, per the
mapping:

    lookup        -> Check
    prepare_store -> Check (filter) + Reserve (best-effort: store the subset
                     that fits, drop blocks the saturated tier can't reserve)
    complete_store-> CommitStore (success) / AbortStore (failure)
    prepare_load  -> Pin(promote=true)
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
        from .compat import lookup_result

        int_key = _key_to_u64(key)
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
        session_id = _session_id_to_u64(req_context)

        # Filter out keys already cached using the local presence cache.
        # No RPC needed — we track what we stored and what was evicted.
        to_store_pairs = [
            (orig, k) for orig, k in zip(keys_list, int_keys)
            if k not in self._presence
        ]

        if not to_store_pairs:
            return PrepareStoreOutput(
                keys_to_store=[],
                store_spec=CertusLoadStoreSpec([]),
                evicted_keys=[],
            )

        to_store_ints = [k for _, k in to_store_pairs]

        # Reserve DRAM slots (server evicts LRU internally to make room). A
        # per-key failure means the server could not free enough space for that
        # block — the memory tier is saturated (e.g. capped at its max size with
        # a working set larger than it, or the evictable set is small because
        # entries are pinned by in-flight loads or not yet written through to
        # SSD). Reserve is per-key independent, so we treat this as best-effort:
        # keep the keys that reserved and drop the rest, rather than rejecting
        # the whole request. A whole-request reject (return None) does NOT
        # advance vLLM's stored-block index, so the same blocks are retried
        # every scheduler step, each logging "cannot store blocks" — a warning
        # storm under sustained pressure. Storing the subset that fits advances
        # the index and offloads what it can.
        reserve = self._stub.Reserve(
            pb.BatchReserveRequest(
                entries=[
                    pb.ReserveEntry(
                        key=k, size=self._block_size_bytes, session_id=session_id
                    )
                    for k in to_store_ints
                ]
            )
        )
        reserved_ok = {r.key for r in reserve.results if r.success}

        # Keep reserved keys in the original offload order (the scheduler
        # positionally zips src GPU block ids with dst keys, deriving src order
        # from offload_keys filtered by keys_to_store — so store_spec must stay
        # in offload order for a partial subset to line up).
        stored_pairs = [
            (orig, k) for orig, k in to_store_pairs if k in reserved_ok
        ]
        dropped = len(to_store_pairs) - len(stored_pairs)
        if dropped:
            self._note_store_drops(dropped)

        # Keys that reserved but that we are not going to store this round would
        # leak a reservation; here every reserved key IS kept (we only drop keys
        # whose Reserve failed, which allocated nothing), so no rollback needed.

        if not stored_pairs:
            # Nothing fit. Return an empty (non-None) result so vLLM advances
            # past these tokens quietly instead of retrying + warning every step.
            return PrepareStoreOutput(
                keys_to_store=[],
                store_spec=CertusLoadStoreSpec([]),
                evicted_keys=[],
            )

        stored_orig = [orig for orig, _ in stored_pairs]
        stored_ints = [k for _, k in stored_pairs]
        locations = [BlockLocation(key=k) for k in stored_ints]
        return PrepareStoreOutput(
            keys_to_store=stored_orig,
            store_spec=CertusLoadStoreSpec(locations),
            # Evictions are surfaced asynchronously via take_events()/TakeEvents.
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
        if success:
            self._stub.CommitStore(pb.BatchCommitStoreRequest(keys=int_keys))
            self._presence.update(int_keys)
        else:
            self._stub.AbortStore(pb.BatchAbortStoreRequest(keys=int_keys))

    # ── load ──

    def prepare_load(self, keys: Iterable[OffloadKey], req_context=None) -> LoadStoreSpec:
        int_keys = _keys_to_u64s(keys)
        # Pin (promote=FALSE) only takes the eviction-protecting read-ref. We must
        # NOT ask Pin to promote: Pin's promote is async/fire-and-forget, and the
        # Lookup that immediately follows (in the load handler) already promotes
        # cold (BlockDevice) entries itself. Two promotes race on the same key —
        # both do mt.insert() — and the loser hits MemoryTierError::AlreadyExists,
        # surfaced as ALLOCATION_FAILED, which fails the load and crashes vLLM
        # (worker asserts transfer success). Lookup is self-sufficient: it serves
        # MemoryTier hits directly and promotes BlockDevice misses in one path.
        resp = self._stub.Pin(pb.BatchPinRequest(keys=int_keys, promote=False))
        # Diagnostic: vLLM only reaches here for keys lookup()/Check reported as
        # present, and cannot drop keys from the returned spec (dst block ids are
        # positionally zipped). So a Pin failure here is the earliest signal that
        # a Check-hit entry vanished — log which key + why.
        for r in resp.results:
            if not r.success:
                print(
                    f"[certus-grpc] PIN FAILURE in prepare_load key={r.key} "
                    f"error_code={r.error_code} msg={r.error_message!r} "
                    f"(Check said present, Pin says gone — eviction race)",
                    flush=True,
                )
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
            # Lost events mean our presence cache may be stale. Clear all
            # positive entries so lookup() falls back to RPC for unknowns.
            self._presence.clear()
        removed = []
        for e in resp.events:
            if e.reason == pb.EVICTION_REASON_REMOVED:
                self._presence.discard(e.key)
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

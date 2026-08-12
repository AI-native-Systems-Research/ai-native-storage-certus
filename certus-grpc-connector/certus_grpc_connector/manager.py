# SPDX-License-Identifier: Apache-2.0
"""OffloadingManager backed by a remote certus-server over gRPC.

All index/allocation/eviction state lives in the server. This class adapts
between vLLM's Python types and the server's u64-keyed batch RPCs, per the
mapping:

    lookup        -> Check
    prepare_store -> Check (filter) + Reserve (best-effort: store the subset
                     that fits, drop blocks the saturated tier can't reserve)
    complete_store-> CommitStore (success) / AbortStore (failure)
    prepare_load  -> Pin(promote=false)
    complete_load -> Unpin
    touch         -> Touch
    take_events   -> TakeEvents(max_events=0)
"""

from __future__ import annotations

import hashlib
import os
import sys
from collections.abc import Iterable, Sequence

from .compat import (
    CAPS,
    LoadStoreSpec,
    OffloadingEvent,
    OffloadingManager,
    OffloadKey,
    PrepareStoreOutput,
)

from . import dispatcher_pb2 as pb
from .mediums import BlockLocation, CertusLoadStoreSpec
from .telemetry import call_rpc


def _env_flag(name: str, default: str = "0") -> bool:
    value = os.environ.get(name, default).strip().lower()
    return value not in {"", "0", "false", "no", "off"}


def _key_to_u64(key: OffloadKey) -> int:
    """Convert an OffloadKey (bytes) to a u64 for the server."""
    if isinstance(key, int):
        return key
    return int.from_bytes(key[:8], "big")


def _keys_to_u64s(keys: Iterable[OffloadKey]) -> list[int]:
    return [_key_to_u64(k) for k in keys]


def _key_debug(key: OffloadKey) -> str:
    if isinstance(key, int):
        return str(key)
    return bytes(key).hex()


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
        # Certus is keyed by u64, while vLLM's OffloadKey can be a longer bytes
        # value. Keep the wire key narrow for server compatibility, but remember
        # the original key identity so eviction events reported back to vLLM match
        # the keys it indexed.
        self._original_keys_by_u64: dict[int, OffloadKey] = {}
        self._collision_keys_logged: set[int] = set()
        # Local presence hints avoid redundant server Check RPCs on the store path.
        # They are hints, not the authority: lookup() still verifies with the
        # server before reporting a hit to vLLM.
        self._known_present_u64: set[int] = set()
        self._known_absent_u64: set[int] = set()
        self._pending_store_u64: set[int] = set()
        self._assume_lookup_hit = _env_flag("CERTUS_GRPC_ASSUME_LOOKUP_HIT", "0")
        # Experimental optimization: use Pin(promote=false) as lookup's
        # authoritative hit test, then consume that read-ref in prepare_load.
        # It is opt-in because it trades Check's per-key RPCs for Pin's per-key
        # RPCs and must be benchmarked on the target server/workload.
        self._pin_on_lookup = _env_flag("CERTUS_GRPC_PIN_ON_LOOKUP", "0")
        self._lookup_pins_by_req: dict[str, dict[OffloadKey, int]] = {}
        # Cumulative count of blocks we could not offload because the server's
        # memory tier was saturated (Reserve failed). Logged in throttled
        # summaries rather than per-request, so a persistently-full tier does
        # not produce a warning storm.
        self._store_dropped_blocks = 0
        self._store_drop_log_next = 1000

    def set_block_size_bytes(self, block_size_bytes: int) -> None:
        """Update the per-block Reserve size once the true KV-cache tensor
        stride is known (the manager is constructed before get_handlers can
        resolve it). Reserve sizes are per-call, so changing this affects only
        subsequent stores."""
        self._block_size_bytes = int(block_size_bytes)

    def _remember_key(self, key: OffloadKey, int_key: int) -> bool:
        """Record the original vLLM key for a Certus u64 key.

        Returns False if a different vLLM key already owns this u64. We then skip
        caching the colliding key rather than risking a false hit to the wrong KV
        block. Collisions should be vanishingly rare for the current workload, so
        this stays a correctness guard instead of changing the server protocol.
        """
        existing = self._original_keys_by_u64.get(int_key)
        if existing is None:
            self._original_keys_by_u64[int_key] = key
            return True
        if existing == key:
            return True
        if int_key not in self._collision_keys_logged:
            self._collision_keys_logged.add(int_key)
            print(
                f"[certus-grpc] WARNING: OffloadKey collision on Certus u64 "
                f"key={int_key}: existing={_key_debug(existing)} "
                f"new={_key_debug(key)}. The new key will not be cached.",
                file=sys.stderr,
                flush=True,
            )
        return False

    def _forget_key(self, key: OffloadKey, int_key: int) -> None:
        if self._original_keys_by_u64.get(int_key) == key:
            self._original_keys_by_u64.pop(int_key, None)
            self._known_present_u64.discard(int_key)
            self._known_absent_u64.discard(int_key)
            self._pending_store_u64.discard(int_key)

    def _clear_presence_hint(self, int_key: int) -> None:
        self._known_present_u64.discard(int_key)
        self._known_absent_u64.discard(int_key)

    def _mark_present(self, key: OffloadKey, int_key: int) -> bool:
        if not self._remember_key(key, int_key):
            return False
        self._known_absent_u64.discard(int_key)
        self._known_present_u64.add(int_key)
        return True

    def _mark_absent(self, key: OffloadKey, int_key: int) -> bool:
        if not self._remember_key(key, int_key):
            return False
        self._known_present_u64.discard(int_key)
        self._known_absent_u64.add(int_key)
        return True

    def _apply_check_result(self, key: OffloadKey, int_key: int, exists: bool) -> bool:
        if exists:
            original = self._original_keys_by_u64.get(int_key)
            if original is None:
                # Preserve the old fast path for entries already present on the
                # server before this manager saw their store. Once observed, keep
                # the original key so future events use vLLM's identity.
                self._mark_present(key, int_key)
            elif original != key:
                self._remember_key(key, int_key)
                exists = False
            else:
                self._mark_present(key, int_key)
        else:
            self._mark_absent(key, int_key)
        return exists

    def _known_absent_lookup(self, key: OffloadKey, int_key: int) -> bool | None:
        if int_key not in self._known_absent_u64 or int_key in self._pending_store_u64:
            return None
        original = self._original_keys_by_u64.get(int_key)
        if original is None:
            self._mark_absent(key, int_key)
            return False
        if original == key:
            return False
        self._remember_key(key, int_key)
        return False

    def _sync_lookup_exists(self, key: OffloadKey, int_key: int) -> bool:
        resp = call_rpc(
            self._stub,
            "Check",
            pb.BatchCheckRequest(keys=[int_key]),
            items=1,
        )
        exists = bool(resp.results and resp.results[0].exists)
        return self._apply_check_result(key, int_key, exists)

    def _pin_lookup_exists(self, key: OffloadKey, int_key: int, req_id: str) -> bool:
        req_pins = self._lookup_pins_by_req.setdefault(req_id, {})
        pinned_key = req_pins.get(key)
        if pinned_key is not None:
            return pinned_key == int_key

        original = self._original_keys_by_u64.get(int_key)
        if original is not None and original != key:
            self._remember_key(key, int_key)
            return False

        resp = call_rpc(
            self._stub,
            "Pin",
            pb.BatchPinRequest(keys=[int_key], promote=False),
            items=1,
        )
        result = resp.results[0] if resp.results else None
        if result is None or not result.success:
            if result is not None and result.error_code == pb.ERROR_CODE_KEY_NOT_FOUND:
                self._mark_absent(key, int_key)
            else:
                self._clear_presence_hint(int_key)
            return False

        if not self._mark_present(key, int_key):
            self._unpin_int_keys([int_key])
            return False

        req_pins[key] = int_key
        return True

    def _req_id(self, req_context) -> str | None:
        req_id = getattr(req_context, "req_id", None)
        return str(req_id) if req_id is not None else None

    def _unpin_int_keys(self, int_keys: list[int]) -> None:
        if int_keys:
            call_rpc(
                self._stub,
                "Unpin",
                pb.BatchUnpinRequest(keys=int_keys),
                items=len(int_keys),
            )

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
        # Returns ``bool`` on ≤0.24 and a ``LookupResult`` enum (HIT/MISS) on
        # 0.26+, which rewrote ``lookup``'s return type. The shim absorbs the
        # difference so this body stays a single Check RPC.
        from .compat import lookup_result

        int_key = _key_to_u64(key)
        if self._assume_lookup_hit:
            self._mark_present(key, int_key)
            return lookup_result(True)

        known_absent = self._known_absent_lookup(key, int_key)
        if known_absent is not None:
            return lookup_result(known_absent)

        req_id = self._req_id(req_context)
        if self._pin_on_lookup and req_id is not None:
            exists = self._pin_lookup_exists(key, int_key, req_id)
            return lookup_result(exists)

        exists = self._sync_lookup_exists(key, int_key)
        return lookup_result(exists)

    def lookup_many(
        self, keys: Sequence[OffloadKey], req_context=None
    ) -> Sequence[object]:
        from .compat import lookup_result

        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)
        results: list[bool | None] = [None] * len(keys_list)
        check_pairs: list[tuple[int, OffloadKey, int]] = []

        for idx, (key, int_key) in enumerate(zip(keys_list, int_keys)):
            if self._assume_lookup_hit:
                self._mark_present(key, int_key)
                results[idx] = True
                continue

            known_absent = self._known_absent_lookup(key, int_key)
            if known_absent is not None:
                results[idx] = known_absent
                continue

            original = self._original_keys_by_u64.get(int_key)
            if original is not None and original != key:
                self._remember_key(key, int_key)
                results[idx] = False
                continue

            check_pairs.append((idx, key, int_key))

        if check_pairs:
            resp = call_rpc(
                self._stub,
                "Check",
                pb.BatchCheckRequest(keys=[int_key for _, _, int_key in check_pairs]),
                items=len(check_pairs),
            )
            exists_by_key = {r.key: bool(r.exists) for r in resp.results}
            for idx, key, int_key in check_pairs:
                exists = exists_by_key.get(int_key, False)
                results[idx] = self._apply_check_result(key, int_key, exists)

        return [lookup_result(bool(result)) for result in results]

    def touch(self, keys: Iterable[OffloadKey], req_context=None) -> None:
        int_keys = _keys_to_u64s(keys)
        if int_keys:
            call_rpc(
                self._stub,
                "Touch",
                pb.BatchTouchRequest(keys=int_keys, promote=False),
                items=len(int_keys),
            )

    # ── store ──

    def prepare_store(
        self, keys: Iterable[OffloadKey], req_context=None
    ) -> PrepareStoreOutput | None:
        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)
        session_id = _session_id_to_u64(req_context)

        # Filter out keys already cached (consecutive dedup is vLLM's concern;
        # here we just avoid re-storing existing entries). Most store candidates
        # were lookup misses earlier in the same request/step; for those, the
        # local "known absent" hint lets us skip a redundant Check RPC.
        to_store_pairs = []
        check_pairs = []
        for orig, k in zip(keys_list, int_keys):
            known = self._original_keys_by_u64.get(k)
            if known is not None and known != orig:
                self._remember_key(orig, k)
                continue
            if k in self._known_present_u64:
                continue
            if k in self._known_absent_u64:
                if self._remember_key(orig, k):
                    to_store_pairs.append((orig, k))
                continue
            check_pairs.append((orig, k))

        if check_pairs:
            check = call_rpc(
                self._stub,
                "Check",
                pb.BatchCheckRequest(keys=[k for _, k in check_pairs]),
                items=len(check_pairs),
            )
            exists = {r.key: r.exists for r in check.results}
            for orig, k in check_pairs:
                if exists.get(k, False):
                    self._mark_present(orig, k)
                    continue
                if not self._mark_absent(orig, k):
                    continue
                to_store_pairs.append((orig, k))

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
        reserve = call_rpc(
            self._stub,
            "Reserve",
            pb.BatchReserveRequest(
                entries=[
                    pb.ReserveEntry(
                        key=k, size=self._block_size_bytes, session_id=session_id
                    )
                    for k in to_store_ints
                ]
            ),
            items=len(to_store_ints),
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
        for k in stored_ints:
            self._known_absent_u64.discard(k)
            self._pending_store_u64.add(k)
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
        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)
        if not int_keys:
            return
        if success:
            resp = call_rpc(
                self._stub,
                "CommitStore",
                pb.BatchCommitStoreRequest(keys=int_keys),
                items=len(int_keys),
            )
            committed = {r.key for r in resp.results if r.success}
            for key, int_key in zip(keys_list, int_keys):
                self._pending_store_u64.discard(int_key)
                if int_key in committed:
                    self._mark_present(key, int_key)
                else:
                    # CommitStore failures can mean "not committed" or "already
                    # present"; do not convert either into a trusted local hint.
                    self._clear_presence_hint(int_key)
        else:
            resp = call_rpc(
                self._stub,
                "AbortStore",
                pb.BatchAbortStoreRequest(keys=int_keys),
                items=len(int_keys),
            )
            aborted = {r.key for r in resp.results if r.success}
            for key, int_key in zip(keys_list, int_keys):
                self._pending_store_u64.discard(int_key)
                if int_key in aborted:
                    self._mark_absent(key, int_key)
                else:
                    self._clear_presence_hint(int_key)

    # ── load ──

    def prepare_load(self, keys: Iterable[OffloadKey], req_context=None) -> LoadStoreSpec:
        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)
        req_id = self._req_id(req_context)
        req_pins = (
            self._lookup_pins_by_req.get(req_id)
            if self._pin_on_lookup and req_id is not None
            else None
        )
        to_pin: list[int] = []
        for key, int_key in zip(keys_list, int_keys):
            if req_pins is not None and req_pins.get(key) == int_key:
                del req_pins[key]
                continue
            to_pin.append(int_key)
        if req_id is not None and req_pins == {}:
            self._lookup_pins_by_req.pop(req_id, None)

        # Pin (promote=FALSE) only takes the eviction-protecting read-ref. We must
        # NOT ask Pin to promote: Pin's promote is async/fire-and-forget, and the
        # Lookup that immediately follows (in the load handler) already promotes
        # cold (BlockDevice) entries itself. Two promotes race on the same key —
        # both do mt.insert() — and the loser hits MemoryTierError::AlreadyExists,
        # surfaced as ALLOCATION_FAILED, which fails the load and crashes vLLM
        # (worker asserts transfer success). Lookup is self-sufficient: it serves
        # MemoryTier hits directly and promotes BlockDevice misses in one path.
        resp = None
        if to_pin:
            resp = call_rpc(
                self._stub,
                "Pin",
                pb.BatchPinRequest(keys=to_pin, promote=False),
                items=len(to_pin),
            )
        # Diagnostic: vLLM only reaches here for keys lookup()/Check reported as
        # present, and cannot drop keys from the returned spec (dst block ids are
        # positionally zipped). So a Pin failure here is the earliest signal that
        # a Check-hit entry vanished — log which key + why.
        if resp is not None:
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
        self._unpin_int_keys(int_keys)

    # ── events / shutdown ──

    def take_events(self) -> Iterable[OffloadingEvent]:
        resp = call_rpc(
            self._stub,
            "TakeEvents",
            pb.TakeEventsRequest(max_events=0),
            items=0,
        )
        if resp.dropped_count:
            print(
                f"[certus-grpc] WARNING: {resp.dropped_count} eviction events "
                "dropped by server (event view is lossy)",
                file=sys.stderr,
                flush=True,
            )
        # Only REMOVED means the key is no longer accessible. DEMOTED entries
        # stay on SSD and remain loadable, so they are not eviction events for
        # vLLM's accounting.
        removed = []
        for e in resp.events:
            if e.reason != pb.EVICTION_REASON_REMOVED:
                continue
            original = self._original_keys_by_u64.pop(e.key, None)
            self._known_present_u64.discard(e.key)
            self._known_absent_u64.add(e.key)
            self._pending_store_u64.discard(e.key)
            removed.append(
                original if original is not None else e.key.to_bytes(8, "big")
            )
        if removed:
            yield OffloadingEvent(
                keys=removed,
                medium=CertusLoadStoreSpec.medium(),
                removed=True,
            )

    def on_request_finished(self, req_context=None) -> None:
        req_id = self._req_id(req_context)
        if req_id is not None:
            pins = self._lookup_pins_by_req.pop(req_id, None)
            if pins:
                self._unpin_int_keys(list(pins.values()))

    def shutdown(self) -> None:
        for req_id in list(self._lookup_pins_by_req):
            pins = self._lookup_pins_by_req.pop(req_id, None)
            if pins:
                self._unpin_int_keys(list(pins.values()))

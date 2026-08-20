# SPDX-License-Identifier: Apache-2.0
"""OffloadingManager backed by a remote certus-server over shared memory.

Identical in behaviour to the gRPC connector's manager — only the transport
changes: every ``self._stub.X(pb.Y(...))`` gRPC call becomes a ``self._ring.x``
call over the ``/dev/shm`` mailbox (``ring.py``). The op → lifecycle mapping is
unchanged:

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
import sys
from collections.abc import Iterable

from .compat import (
    LoadStoreSpec,
    OffloadingEvent,
    OffloadingManager,
    OffloadKey,
    PrepareStoreOutput,
)

from .mediums import BlockLocation, CertusLoadStoreSpec
from .ring import REASON_REMOVED


def _key_to_u64(key: OffloadKey) -> int:
    """Convert an OffloadKey (bytes) to a u64 for the server.

    The OffloadKey is vLLM's ``block_hash + group_idx`` (36 bytes for the
    default SHA-256 block hash: 32-byte digest + 4-byte big-endian group
    index). All bytes are folded into the u64 with a stable hash (BLAKE2b,
    first 8 bytes big-endian) so the derived key reflects the full block
    identity and the KV-cache group. Truncating to ``key[:8]`` would drop 24
    hash bytes and ignore the group index entirely, aliasing distinct blocks.
    """
    if isinstance(key, int):
        return key
    digest = hashlib.blake2b(bytes(key), digest_size=8).digest()
    return int.from_bytes(digest, "big")


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


class ShmqCertusOffloadingManager(OffloadingManager):
    """Manager delegating to a remote certus-server via a shared-memory ring."""

    def __init__(self, ring, block_size_bytes: int):
        self._ring = ring
        self._block_size_bytes = int(block_size_bytes)
        # Cumulative count of blocks we could not offload because the server's
        # memory tier was saturated (Reserve failed). Logged in throttled
        # summaries rather than per-request, so a persistently-full tier does
        # not produce a warning storm.
        self._store_dropped_blocks = 0
        self._store_drop_log_next = 1000
        # Per-pass lookup cache. The scheduler calls ``touch(full_key_list)`` for
        # every KV group and THEN runs its maximal-prefix lookup loop over a
        # slice of those same keys — one ``lookup()`` per key, breaking at the
        # first miss (offloading/scheduler.py::_maximal_prefix_lookup). We fire a
        # single batched ``Check`` inside ``touch`` (which already ships the whole
        # key list) and answer each per-key ``lookup`` from this map, collapsing
        # up to K per-key ``Check`` round-trips into one. Scope is a single
        # scheduling pass: a ``touch`` that follows a ``lookup`` starts a new pass
        # and clears the map (so a positive bit can never be reused across steps,
        # after the key may have been evicted). A cache miss falls back to the
        # authoritative single-key ``Check`` — an unseen key is never answered
        # wrong, only un-batched. Values are raw Check *states* (miss/resident/
        # pending), not bools, so a pending store surfaces as HIT_PENDING on 0.26.
        self._lookup_cache: dict[int, int] = {}
        self._last_op_was_lookup = False

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
        # Returns ``bool`` on ≤0.24 and a ``LookupResult`` enum
        # (MISS/HIT/HIT_PENDING) on 0.26+, which rewrote ``lookup``'s return type.
        # The shim maps the raw Check state to whichever this version expects, so
        # this body stays a single Check call.
        from .compat import lookup_result
        from .ring import CHECK_MISS

        self._last_op_was_lookup = True
        int_key = _key_to_u64(key)
        # Fast path: answer from the state map the preceding touch() batched. A
        # miss (key not in this pass's batch, e.g. the scheduler looking up a
        # key it never touched) falls back to the authoritative single-key
        # Check — correctness is never traded for the batch, only latency.
        state = self._lookup_cache.get(int_key)
        if state is None:
            states = self._ring.check_states([int_key])
            state = states[0] if states else CHECK_MISS
        return lookup_result(state)

    def touch(self, keys: Iterable[OffloadKey], req_context=None) -> None:
        # A touch that follows a lookup opens a new scheduling pass — retire the
        # previous pass's bitmap so a stale positive can never outlive the step
        # in which its Check was authoritative (the key may since be evicted).
        if self._last_op_was_lookup:
            self._lookup_cache.clear()
            self._last_op_was_lookup = False
        int_keys = _keys_to_u64s(keys)
        if not int_keys:
            return
        self._ring.touch(int_keys, promote=False)
        # Batch the existence probe for the whole key list here, where we
        # already hold it, so the scheduler's subsequent per-key lookup loop
        # (offloading/scheduler.py::_maximal_prefix_lookup) is served from this
        # map instead of firing one Check RPC per key. Cache the tri-state so a
        # pending store carries through to HIT_PENDING at lookup time.
        states = self._ring.check_states(int_keys)
        self._lookup_cache.update(zip(int_keys, states))

    # ── store ──

    def prepare_store(
        self, keys: Iterable[OffloadKey], req_context=None
    ) -> PrepareStoreOutput | None:
        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)
        session_id = _session_id_to_u64(req_context)

        # Filter out keys already cached (consecutive dedup is vLLM's concern;
        # here we just avoid re-storing existing entries).
        exists_flags = self._ring.check(int_keys)
        exists = {k: e for k, e in zip(int_keys, exists_flags)}
        to_store_pairs = [
            (orig, k) for orig, k in zip(keys_list, int_keys) if not exists.get(k, False)
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
        reserved_flags = self._ring.reserve(
            [(k, self._block_size_bytes, session_id) for k in to_store_ints]
        )
        reserved_ok = {k for k, ok in zip(to_store_ints, reserved_flags) if ok}

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
                f"[certus-shmq] memory tier saturated: skipped offloading "
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
            self._ring.commit_store(int_keys)
        else:
            self._ring.abort_store(int_keys)

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
        pin_ok = self._ring.pin(int_keys, promote=False)
        # Diagnostic: vLLM only reaches here for keys lookup()/Check reported as
        # present, and cannot drop keys from the returned spec (dst block ids are
        # positionally zipped). So a Pin failure here is the earliest signal that
        # a Check-hit entry vanished — log which key.
        for key, ok in zip(int_keys, pin_ok):
            if not ok:
                print(
                    f"[certus-shmq] PIN FAILURE key={key} "
                    f"(Check said present, Pin says gone — eviction race)",
                    flush=True,
                )
        return CertusLoadStoreSpec([BlockLocation(key=k) for k in int_keys])

    def complete_load(self, keys: Iterable[OffloadKey], req_context=None) -> None:
        int_keys = _keys_to_u64s(keys)
        if int_keys:
            self._ring.unpin(int_keys)

    # ── events / shutdown ──

    def take_events(self) -> Iterable[OffloadingEvent]:
        events, dropped = self._ring.take_events(0)
        if dropped:
            print(
                f"[certus-shmq] WARNING: {dropped} eviction events "
                "dropped by server (event view is lossy)",
                file=sys.stderr,
                flush=True,
            )
        # Only REMOVED means the key is no longer accessible. DEMOTED entries
        # stay on SSD and remain loadable, so they are not eviction events for
        # vLLM's accounting.
        removed = [
            key.to_bytes(8, "big") for key, reason in events if reason == REASON_REMOVED
        ]
        if removed:
            yield OffloadingEvent(
                keys=removed,
                medium=CertusLoadStoreSpec.medium(),
                removed=True,
            )

    def shutdown(self) -> None:
        # Ring is owned by the spec singleton; nothing per-manager to close.
        pass

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

from .mediums import BlockLocation, CertusLoadStoreSpec, denamespace_key, ns_key
from .ring import REASON_REMOVED


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


class ShmqCertusOffloadingManager(OffloadingManager):
    """Manager delegating to a remote certus-server via a shared-memory ring."""

    def __init__(self, ring, block_size_bytes: int, world_size: int = 1):
        self._ring = ring
        self._block_size_bytes = int(block_size_bytes)
        # Number of TP shards. Under TP>1 each logical vLLM block is physically W
        # separate per-rank entries in the shared server (one head-shard each,
        # under keys namespaced by rank — see mediums.ns_key). This scheduler-side
        # manager mirrors what the W workers store: every server op expands a
        # logical key K into {ns_key(K, r) : r in range(W)}, and residency is
        # AND-across-ranks (a block is loadable only if EVERY shard is present).
        # W==1 → ns_key is identity and each expansion is [K] → exact baseline.
        self._world_size = max(1, int(world_size))
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
        # wrong, only un-batched.
        self._lookup_cache: dict[int, bool] = {}
        self._last_op_was_lookup = False

    def set_block_size_bytes(self, block_size_bytes: int) -> None:
        """Update the per-block Reserve size once the true KV-cache tensor
        stride is known (the manager is constructed before get_handlers can
        resolve it). Reserve sizes are per-call, so changing this affects only
        subsequent stores."""
        self._block_size_bytes = int(block_size_bytes)

    # ── per-rank key expansion (TP>1) ──

    def _ns_all(self, logical_key: int) -> list[int]:
        """The W physical per-rank server keys for one logical block. At W==1
        this is ``[logical_key]`` (ns_key is identity) → exact baseline."""
        return [ns_key(logical_key, r, self._world_size) for r in range(self._world_size)]

    def _check_all_present(self, logical_keys: list[int]) -> dict[int, bool]:
        """Batched Check over every per-rank key. A logical block is present iff
        ALL W of its shards are present — a load needs every shard, so partial
        residency (some ranks evicted) must read as a MISS. One batched Check
        RPC covers the whole expanded list; flags return in expansion order."""
        w = self._world_size
        if not logical_keys:
            return {}
        expanded = [nk for k in logical_keys for nk in self._ns_all(k)]
        flags = self._ring.check(expanded)
        out: dict[int, bool] = {}
        for i, k in enumerate(logical_keys):
            chunk = flags[i * w:(i + 1) * w]
            out[k] = len(chunk) == w and all(chunk)
        return out

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
        # difference so this body stays a single Check call.
        from .compat import lookup_result

        self._last_op_was_lookup = True
        int_key = _key_to_u64(key)
        # Fast path: answer from the bitmap the preceding touch() batched. A
        # miss (key not in this pass's batch, e.g. the scheduler looking up a
        # key it never touched) falls back to the authoritative single-key
        # Check — correctness is never traded for the batch, only latency.
        cached = self._lookup_cache.get(int_key)
        if cached is None:
            # AND across all W shards — a load needs every rank's shard present.
            cached = self._check_all_present([int_key]).get(int_key, False)
        return lookup_result(cached)

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
        # Touch every per-rank key so all W shards' LRU positions advance.
        expanded = [nk for k in int_keys for nk in self._ns_all(k)]
        self._ring.touch(expanded, promote=False)
        # Batch the existence probe for the whole key list here, where we
        # already hold it, so the scheduler's subsequent per-key lookup loop
        # (offloading/scheduler.py::_maximal_prefix_lookup) is served from this
        # map instead of firing one Check RPC per key. AND-across-ranks per
        # logical key (a block is a hit only if all shards are present).
        self._lookup_cache.update(self._check_all_present(int_keys))

    # ── store ──

    def prepare_store(
        self, keys: Iterable[OffloadKey], req_context=None
    ) -> PrepareStoreOutput | None:
        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)
        session_id = _session_id_to_u64(req_context)

        # Filter out keys already cached (consecutive dedup is vLLM's concern;
        # here we just avoid re-storing existing entries). "Already cached" means
        # ALL W shards present; a partially-resident block (some ranks evicted)
        # is re-stored so every shard is repopulated together.
        exists = self._check_all_present(int_keys)
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
        # Reserve ALL W per-rank shards of each block. A logical block is storable
        # only if EVERY shard reserves (else a load — which needs all W — could
        # never succeed): all-or-nothing per block. One batched Reserve covers the
        # expanded list; flags return in expansion order.
        w = self._world_size
        reserve_reqs = [
            (nk, self._block_size_bytes, session_id)
            for k in to_store_ints
            for nk in self._ns_all(k)
        ]
        reserved_flags = self._ring.reserve(reserve_reqs)
        reserved_ok: set[int] = set()
        orphan_aborts: list[int] = []  # partial reserves to roll back
        for i, k in enumerate(to_store_ints):
            chunk = reserved_flags[i * w:(i + 1) * w]
            if len(chunk) == w and all(chunk):
                reserved_ok.add(k)
            else:
                # Some shards reserved, some didn't: abort the ones that DID so a
                # half-reserved block can't leak or later publish as a false hit.
                for r, ok in enumerate(chunk):
                    if ok:
                        orphan_aborts.append(ns_key(k, r, w))
        if orphan_aborts:
            try:
                self._ring.abort_store(orphan_aborts)
            except Exception:  # noqa: BLE001 - best-effort rollback
                pass

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
        # Commit/abort every per-rank shard the W workers stored under this key.
        expanded = [nk for k in int_keys for nk in self._ns_all(k)]
        if success:
            self._ring.commit_store(expanded)
        else:
            self._ring.abort_store(expanded)

    # ── load ──

    def prepare_load(self, keys: Iterable[OffloadKey], req_context=None) -> LoadStoreSpec:
        int_keys = _keys_to_u64s(keys)
        # Pin every per-rank shard: each of the W workers loads its own shard, so
        # all W must be eviction-protected for the duration of the load. The
        # returned spec carries LOGICAL keys — each worker folds in its own rank.
        pin_keys = [nk for k in int_keys for nk in self._ns_all(k)]
        # Pin (promote=FALSE) only takes the eviction-protecting read-ref. We must
        # NOT ask Pin to promote: Pin's promote is async/fire-and-forget, and the
        # Lookup that immediately follows (in the load handler) already promotes
        # cold (BlockDevice) entries itself. Two promotes race on the same key —
        # both do mt.insert() — and the loser hits MemoryTierError::AlreadyExists,
        # surfaced as ALLOCATION_FAILED, which fails the load and crashes vLLM
        # (worker asserts transfer success). Lookup is self-sufficient: it serves
        # MemoryTier hits directly and promotes BlockDevice misses in one path.
        pin_ok = self._ring.pin(pin_keys, promote=False)
        # Diagnostic: vLLM only reaches here for keys lookup()/Check reported as
        # present, and cannot drop keys from the returned spec (dst block ids are
        # positionally zipped). So a Pin failure here is the earliest signal that
        # a Check-hit entry vanished — log which (namespaced) key.
        for nk, ok in zip(pin_keys, pin_ok):
            if not ok:
                print(
                    f"[certus-shmq] PIN FAILURE key={nk} "
                    f"(Check said present, Pin says gone — eviction race)",
                    flush=True,
                )
        return CertusLoadStoreSpec([BlockLocation(key=k) for k in int_keys])

    def complete_load(self, keys: Iterable[OffloadKey], req_context=None) -> None:
        int_keys = _keys_to_u64s(keys)
        if int_keys:
            expanded = [nk for k in int_keys for nk in self._ns_all(k)]
            self._ring.unpin(expanded)

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
        # vLLM's accounting. Under TP>1 each logical block is W per-rank keys;
        # map each REMOVED key back to its logical key and dedup so vLLM sees one
        # eviction per logical block. This is conservative-safe: emitting a
        # logical eviction when ANY shard is removed matches lookup()'s AND
        # semantics (a block with a missing shard already reads as a MISS), and
        # the authoritative pre-load Check prevents loading a partially-evicted
        # block regardless — so no reverse map from logical→rank is needed.
        seen: set[int] = set()
        removed: list[bytes] = []
        for key, reason in events:
            if reason != REASON_REMOVED:
                continue
            logical = denamespace_key(key, self._world_size)
            if logical in seen:
                continue
            seen.add(logical)
            removed.append(logical.to_bytes(8, "big"))
        if removed:
            yield OffloadingEvent(
                keys=removed,
                medium=CertusLoadStoreSpec.medium(),
                removed=True,
            )

    def shutdown(self) -> None:
        # Ring is owned by the spec singleton; nothing per-manager to close.
        pass

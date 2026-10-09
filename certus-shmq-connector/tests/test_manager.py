# SPDX-License-Identifier: Apache-2.0
"""Unit tests for the shmq connector logic (no server, no GPU, no vLLM).

conftest.py installs fake vllm modules so these import cleanly. The gRPC
connector's equivalent tests drove a proto-based ``FakeStub``; here a ``FakeRing``
mimics the ``ring.py`` transport surface (per-key bool lists + a take_events
tuple), so the manager/handler logic is exercised against the exact call shapes
they issue.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

import pytest

from certus_shmq_connector.gpu import KvCacheIpc
from certus_shmq_connector.manager import ShmqCertusOffloadingManager, _key_to_u64
from certus_shmq_connector.mediums import BlockLocation, CertusLoadStoreSpec
from certus_shmq_connector.ring import REASON_DEMOTED, REASON_REMOVED


# ── Fake Ring transport ──


class FakeRing:
    """Records calls and returns canned per-key results, mirroring ring.py.

    Every op returns a ``list[bool]`` in key/entry order (as the real Ring does),
    except ``take_events`` which returns ``(events, dropped)``.
    """

    def __init__(self):
        self.calls: list[tuple[str, object]] = []
        self.exists: dict[int, bool] = {}
        # Optional per-key state override (CHECK_MISS/RESIDENT/PENDING). Keys not
        # listed fall back to exists -> RESIDENT/MISS, so exists-based tests are
        # unaffected; set this to exercise the PENDING (HIT_PENDING) path.
        self.states: dict[int, int] = {}
        self.reserve_fail: set[int] = set()
        self.copy_fail: set[int] = set()
        self.events: list[tuple[int, int]] = []
        self.dropped_count = 0

    def _state_of(self, k):
        from certus_shmq_connector.ring import CHECK_MISS, CHECK_RESIDENT

        return self.states.get(
            k, CHECK_RESIDENT if self.exists.get(k, False) else CHECK_MISS
        )

    def check(self, keys):
        from certus_shmq_connector.ring import CHECK_MISS

        keys = list(keys)
        self.calls.append(("check", keys))
        # Existence view, matching ring.check == [s != MISS for s in states].
        return [self._state_of(k) != CHECK_MISS for k in keys]

    def check_states(self, keys):
        keys = list(keys)
        self.calls.append(("check_states", keys))
        return [self._state_of(k) for k in keys]

    def touch(self, keys, promote=False):
        keys = list(keys)
        self.calls.append(("touch", (keys, promote)))
        return [True] * len(keys)

    def touch_states(self, keys, promote=False):
        # Fused TouchCheck: recorded as "touch" so touch-call assertions hold.
        keys = list(keys)
        self.calls.append(("touch", (keys, promote)))
        return [self._state_of(k) for k in keys]

    def reserve(self, entries):
        entries = list(entries)
        self.calls.append(("reserve", entries))
        return [e[0] not in self.reserve_fail for e in entries]

    def commit_store(self, keys):
        keys = list(keys)
        self.calls.append(("commit_store", keys))
        return [True] * len(keys)

    def abort_store(self, keys):
        keys = list(keys)
        self.calls.append(("abort_store", keys))
        return [True] * len(keys)

    def pin(self, keys, promote=False):
        keys = list(keys)
        self.calls.append(("pin", (keys, promote)))
        return [True] * len(keys)

    def unpin(self, keys):
        keys = list(keys)
        self.calls.append(("unpin", keys))
        return [True] * len(keys)

    def copy_to_store(self, entries):
        entries = list(entries)
        self.calls.append(("copy_to_store", entries))
        return [e[0] not in self.copy_fail for e in entries]

    def lookup(self, entries):
        entries = list(entries)
        self.calls.append(("lookup", entries))
        return [True] * len(entries)

    def take_events(self, max_events=0):
        self.calls.append(("take_events", max_events))
        # The real server drains its queue on each call; mirror that so a second
        # call returns nothing.
        events, self.events = self.events, []
        dropped, self.dropped_count = self.dropped_count, 0
        return events, dropped


def _calls_of(ring, name):
    return [args for n, args in ring.calls if n == name]


# ── key mapping ──


def _offload_key(block_hash: bytes, group_idx: int) -> bytes:
    """vLLM OffloadKey layout: 32-byte block hash + 4-byte big-endian group."""
    return block_hash + group_idx.to_bytes(4, "big", signed=False)


def K(n: int) -> bytes:
    """A realistic 36-byte OffloadKey standing in for test block ``n``."""
    return _offload_key(n.to_bytes(32, "big"), 0)


def U(n: int) -> int:
    """The u64 the manager derives for :func:`K` (``n``) — what the ring sees.

    ``_key_to_u64`` hashes the full key, so the ring no longer observes ``n``
    itself; tests seed and assert ring state through this mapping instead.
    """
    return _key_to_u64(K(n))


def test_key_to_u64_ints_pass_through():
    assert _key_to_u64(42) == 42


def test_key_to_u64_is_deterministic_and_fits_u64():
    key = _offload_key(b"\xab" * 32, 3)
    v = _key_to_u64(key)
    assert v == _key_to_u64(key)  # stable
    assert 0 <= v < 2**64


def test_key_to_u64_distinguishes_group_index():
    # Two blocks with the SAME 32-byte hash but different KV-cache groups must
    # not collide — the group-index bytes are re-hashed into the key.
    block_hash = b"\x11" * 32
    assert _key_to_u64(_offload_key(block_hash, 0)) != _key_to_u64(
        _offload_key(block_hash, 1)
    )


def test_key_to_u64_covers_full_hash_not_just_prefix():
    # Keys sharing the first 8 hash bytes but differing later must not collide;
    # the old key[:8] truncation would have aliased these to the same u64.
    prefix = b"\x00" * 8
    a = _offload_key(prefix + b"\x01" + b"\x00" * 23, 0)
    b = _offload_key(prefix + b"\x02" + b"\x00" * 23, 0)
    assert a[:8] == b[:8]  # guard: identical prefix
    assert _key_to_u64(a) != _key_to_u64(b)


# ── offset math (KvCacheIpc) ──


def test_block_offset_includes_base_delta_and_stride():
    kv = KvCacheIpc(handle_bytes=b"h" * 64, gpu_device_id=0, stride_bytes=2048, base_delta=512)
    assert kv.block_offset(0) == 512
    assert kv.block_offset(1) == 512 + 2048
    assert kv.block_offset(5) == 512 + 5 * 2048


# ── manager: lookup / touch ──


def test_lookup_maps_to_check():
    ring = FakeRing()
    ring.exists[U(7)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    assert mgr.lookup(K(7)) is True
    assert mgr.lookup(K(8)) is False


def test_touch_maps_to_touch_no_promote():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), K(2)])
    (args,) = _calls_of(ring, "touch")
    keys, promote = args
    assert keys == [U(1), U(2)]
    assert promote is False


def test_touch_batches_check_for_following_per_key_lookups():
    # touch() ships the whole key list as one fused TouchCheck, whose reply
    # carries per-key residency — so touch() issues NO separate check, and the
    # scheduler's subsequent per-key lookup loop is served from the memoized
    # bitmap — no per-key check RPC either.
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(2)] = True
    # key 3 absent
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [K(1), K(2), K(3)]

    mgr.touch(keys)
    # Exactly one fused touch over the full list, and no check RPC.
    assert _calls_of(ring, "touch") == [([U(1), U(2), U(3)], False)]
    assert _calls_of(ring, "check_states") == []

    # Per-key lookups answer from the cache, issuing NO check.
    assert mgr.lookup(keys[0]) is True
    assert mgr.lookup(keys[1]) is True
    assert mgr.lookup(keys[2]) is False
    assert _calls_of(ring, "check_states") == []


def test_lookup_miss_falls_back_to_single_check():
    # A lookup for a key the current pass never touched must consult the
    # authoritative single-key check rather than answering absent from a stale
    # or empty bitmap.
    ring = FakeRing()
    ring.exists[U(42)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1)])  # bitmap covers key 1 only
    assert _calls_of(ring, "check_states") == []  # residency came from touch
    assert mgr.lookup(K(42)) is True
    # The fallback single-key check happened for the uncached key only.
    assert _calls_of(ring, "check_states") == [[U(42)]]


def test_touch_after_lookup_starts_new_pass_and_clears_bitmap():
    # A touch that follows a lookup opens a new scheduling pass: the prior pass's
    # positive bit must not survive (the key may since have been evicted), so the
    # next lookup re-derives from the fresh batched check.
    ring = FakeRing()
    ring.exists[U(5)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)

    mgr.touch([K(5)])
    assert mgr.lookup(K(5)) is True

    # Key 5 evicted between passes; new pass's batched check reflects it.
    ring.exists[U(5)] = False
    mgr.touch([K(5)])
    assert mgr.lookup(K(5)) is False


def test_lookup_pending_reads_as_miss():
    # A store in flight -> Check PENDING. A PENDING key is Reserve'd but NOT yet
    # committed (no dispatch-map entry), so it is not loadable: the load-decision
    # path (lookup) MUST treat it as a MISS. If lookup reported it present, vLLM
    # would count a cache hit, issue a load, and fatally fail the transfer
    # (Pin/lookup find NotExist -> EngineDeadError -- the observed 60-session
    # saturation crash). Reading PENDING as a miss makes vLLM recompute the
    # momentarily-in-flight block instead. Store dedup keeps PENDING-as-present
    # separately (see test_prepare_store_*), so the in-flight store is not
    # duplicated. Contract: MISS is False on the ≤0.24 bool shim / LookupResult
    # .MISS on 0.26 -- never None/HIT_PENDING. See _check_all_present(resident_only).
    from certus_shmq_connector.ring import CHECK_PENDING

    ring = FakeRing()
    ring.states[U(7)] = CHECK_PENDING
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    assert mgr.lookup(K(7)) is False


def test_touch_caches_pending_state_for_following_lookups():
    # The PENDING state must survive the touch()-batched lookup cache: a PENDING
    # key looked up after touch is answered from the cached state (no extra RPC)
    # and still reads as a MISS, not resident -- the load-decision path never
    # treats PENDING as loadable. See test_lookup_pending_reads_as_miss.
    from certus_shmq_connector.ring import CHECK_PENDING

    ring = FakeRing()
    ring.exists[U(1)] = True  # resident
    ring.states[U(2)] = CHECK_PENDING  # store in flight
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [K(1), K(2)]

    mgr.touch(keys)
    assert _calls_of(ring, "check_states") == []  # residency came from touch
    assert mgr.lookup(keys[0]) is True  # resident
    assert mgr.lookup(keys[1]) is False  # pending -> reads as MISS, from cache
    assert _calls_of(ring, "check_states") == []  # no further RPC


def test_touch_tp_any_non_resident_shard_makes_logical_key_miss():
    # TP>1: touch() expands each logical key to W per-rank shards and a block is
    # a hit only if ALL shards are RESIDENT. One PENDING or MISS shard makes the
    # logical key miss, answered from the cache with no check RPC.
    from certus_shmq_connector.mediums import ns_key
    from certus_shmq_connector.ring import CHECK_PENDING

    w = 2
    ring = FakeRing()
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = True  # all shards resident -> hit
    ring.exists[ns_key(U(2), 0, w)] = True  # rank 1 shard pending -> miss
    ring.states[ns_key(U(2), 1, w)] = CHECK_PENDING
    ring.exists[ns_key(U(3), 1, w)] = True  # rank 0 shard absent -> miss
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=w)
    keys = [K(1), K(2), K(3)]

    mgr.touch(keys)
    ((touched, promote),) = _calls_of(ring, "touch")
    assert touched == [ns_key(U(k), r, w) for k in (1, 2, 3) for r in range(w)]
    assert promote is False
    assert mgr.lookup(keys[0]) is True
    assert mgr.lookup(keys[1]) is False
    assert mgr.lookup(keys[2]) is False
    assert _calls_of(ring, "check_states") == []


def _old_touch_fill(cache, int_keys, w, ring):
    """Reference copy of the pre-bulk touch() expand + flags + per-key fill.
    Returns the touch_states key list it would have sent."""
    from certus_shmq_connector.mediums import ns_key
    from certus_shmq_connector.ring import CHECK_RESIDENT

    expanded = [ns_key(k, r, w) for k in int_keys for r in range(w)]
    flags = [ring._state_of(nk) == CHECK_RESIDENT for nk in expanded]
    for i, k in enumerate(int_keys):
        chunk = flags[i * w:(i + 1) * w]
        cache[k] = len(chunk) == w and all(chunk)
    return expanded


def _seed_mixed_states(ring, int_keys):
    from certus_shmq_connector.ring import CHECK_MISS, CHECK_PENDING, CHECK_RESIDENT

    alphabet = (CHECK_RESIDENT, CHECK_PENDING, CHECK_MISS)
    for i, k in enumerate(int_keys):
        ring.states[k] = alphabet[i % 3]


def test_touch_w1_bulk_fill_matches_per_key_reference_in_order():
    # W=1 skips the expansion and fills the cache with one bulk update. The
    # touch_states key list and list(_lookup_cache.items()) — order included —
    # must equal the old per-key loop, with duplicate keys (last write wins at
    # the first-insertion position) and a pre-existing entry from this pass.
    ring = FakeRing()
    keys = [K(n) for n in (1, 2, 3, 2, 4, 5, 1, 6)]
    int_keys = [U(n) for n in (1, 2, 3, 2, 4, 5, 1, 6)]
    _seed_mixed_states(ring, sorted(set(int_keys)))

    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr._lookup_cache[U(3)] = True  # stale-in-pass entry, overwritten in place
    mgr._lookup_cache[U(9)] = False  # untouched entry, kept

    expected = {U(3): True, U(9): False}
    expected_sent = _old_touch_fill(expected, int_keys, 1, ring)

    mgr.touch(keys)
    ((sent, promote),) = _calls_of(ring, "touch")
    assert sent == expected_sent == int_keys
    assert promote is False
    assert list(mgr._lookup_cache.items()) == list(expected.items())
    assert all(v is True or v is False for v in mgr._lookup_cache.values())


def test_touch_w1_resident_true_pending_and_miss_false():
    from certus_shmq_connector.ring import CHECK_MISS, CHECK_PENDING, CHECK_RESIDENT

    ring = FakeRing()
    ring.states[U(1)] = CHECK_RESIDENT
    ring.states[U(2)] = CHECK_PENDING
    ring.states[U(3)] = CHECK_MISS
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), K(2), K(3)])
    assert list(mgr._lookup_cache.items()) == [(U(1), True), (U(2), False), (U(3), False)]
    for v in mgr._lookup_cache.values():
        assert v is True or v is False


def test_touch_w1_short_states_leave_uncovered_keys_false():
    # Only a fake can return fewer states than keys; uncovered keys must read
    # as a miss, matching the old `len(chunk) == w and all(chunk)`.
    class ShortRing(FakeRing):
        def touch_states(self, keys, promote=False):
            return super().touch_states(keys, promote)[:2]

    ring = ShortRing()
    for n in (1, 2, 3, 4):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), K(2), K(3), K(4)])
    assert list(mgr._lookup_cache.items()) == [
        (U(1), True),
        (U(2), True),
        (U(3), False),
        (U(4), False),
    ]
    for v in mgr._lookup_cache.values():
        assert v is True or v is False


def test_touch_w2_calls_and_cache_unchanged():
    # W>1 keeps the expansion and AND-across-ranks fill; compare with the old
    # per-key reference, duplicates and a pre-existing entry included.
    from certus_shmq_connector.mediums import ns_key
    from certus_shmq_connector.ring import CHECK_PENDING

    w = 2
    ring = FakeRing()
    ns = (1, 2, 3, 4, 2)
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = True  # all resident -> hit
        ring.exists[ns_key(U(4), r, w)] = True
    ring.exists[ns_key(U(2), 0, w)] = True
    ring.states[ns_key(U(2), 1, w)] = CHECK_PENDING  # one pending shard -> miss
    ring.exists[ns_key(U(3), 1, w)] = True  # rank 0 absent -> miss
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=w)
    mgr._lookup_cache[U(4)] = False

    int_keys = [U(n) for n in ns]
    expected = {U(4): False}
    expected_sent = _old_touch_fill(expected, int_keys, w, ring)

    mgr.touch([K(n) for n in ns])
    ((sent, promote),) = _calls_of(ring, "touch")
    assert sent == expected_sent
    assert promote is False
    assert list(mgr._lookup_cache.items()) == list(expected.items())
    assert mgr._lookup_cache[U(1)] is True and mgr._lookup_cache[U(4)] is True


def test_touch_w1_lookup_then_touch_still_clears_pass():
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(2)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), K(2)])
    assert mgr.lookup(K(1)) is True
    mgr.touch([K(3)])  # new pass: prior pass's entries are retired
    assert list(mgr._lookup_cache.items()) == [(U(3), False)]
    ring.exists[U(1)] = False
    assert mgr.lookup(K(1)) is False  # authoritative fallback, not stale True
    assert _calls_of(ring, "check_states") == [[U(1)]]


# ── manager: raw-key lookup fast path ──


class _ReferenceManager(ShmqCertusOffloadingManager):
    """lookup() via the unmodified algorithm: fold, then the u64 map, then the
    single-key Check fallback — never the raw-key map."""

    def lookup(self, key, req_context=None):
        from certus_shmq_connector.compat import lookup_result

        self._last_op_was_lookup = True
        int_key = _key_to_u64(key)
        cached = self._lookup_cache.get(int_key)
        if cached is None:
            cached = self._check_all_present([int_key], resident_only=True).get(
                int_key, False
            )
        return lookup_result(cached)


def _mixed_ring(w):
    """Key 1 resident, 2 pending, 3 missing on every shard; 4 resident on one
    shard only when w > 1."""
    from certus_shmq_connector.mediums import ns_key
    from certus_shmq_connector.ring import CHECK_PENDING

    ring = FakeRing()
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = True
        ring.states[ns_key(U(2), r, w)] = CHECK_PENDING
    ring.exists[ns_key(U(4), 0, w)] = True
    return ring


@pytest.mark.parametrize("w", [1, 2])
def test_raw_lookup_cache_matches_u64_map_and_reference(w):
    keys = [K(1), K(2), K(3), K(4), K(1), K(3)]  # duplicates included
    ring, ref_ring = _mixed_ring(w), _mixed_ring(w)
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=w)
    ref = _ReferenceManager(ref_ring, block_size_bytes=4096, world_size=w)

    mgr.touch(keys)
    ref.touch(keys)
    # Every raw entry is the u64 map's value for the folded key.
    for k in keys:
        assert mgr._raw_lookup_cache[k] is mgr._lookup_cache[_key_to_u64(k)]
    assert mgr._lookup_cache == ref._lookup_cache

    for k in keys:
        assert mgr.lookup(k) == ref.lookup(k)
    # Touched keys never fall back to a Check; the issued calls are identical.
    assert _calls_of(ring, "check_states") == []
    assert ring.calls == ref_ring.calls


def test_raw_lookup_fallbacks_for_untouched_and_non_bytes_keys():
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(42)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), K(2)])

    # Untouched key: exactly one single-key Check.
    assert mgr.lookup(K(42)) is True
    assert _calls_of(ring, "check_states") == [[U(42)]]
    # Unhashable bytearray, memoryview and int keys answer like the bytes key.
    assert mgr.lookup(bytearray(K(1))) is True
    assert mgr.lookup(bytearray(K(2))) is False
    assert mgr.lookup(memoryview(K(1))) is True
    assert mgr.lookup(U(1)) is True
    assert mgr.lookup(U(2)) is False
    assert _calls_of(ring, "check_states") == [[U(42)]]


def test_touch_with_unhashable_key_still_serves_lookups():
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(3)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), bytearray(K(2)), K(3)])
    assert mgr.lookup(K(1)) is True
    assert mgr.lookup(K(2)) is False
    assert mgr.lookup(bytearray(K(2))) is False
    assert mgr.lookup(K(3)) is True
    assert _calls_of(ring, "check_states") == []


def test_touch_after_lookup_clears_both_maps():
    ring = FakeRing()
    ring.exists[U(5)] = True
    ring.exists[U(6)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)

    mgr.touch([K(5), K(6)])
    assert mgr.lookup(K(5)) is True

    # Key 5 evicted between passes; the new pass drops every prior entry.
    ring.exists[U(5)] = False
    mgr.touch([K(5)])
    assert K(6) not in mgr._raw_lookup_cache
    assert U(6) not in mgr._lookup_cache
    assert mgr.lookup(K(5)) is False


def test_lookup_honours_compat_lookup_result_patched_after_construction(monkeypatch):
    import certus_shmq_connector.compat as compat

    ring = FakeRing()
    ring.exists[U(1)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1)])
    sentinel = object()
    monkeypatch.setattr(compat, "lookup_result", lambda hit: (sentinel, hit))
    assert mgr.lookup(K(1)) == (sentinel, True)
    assert mgr.lookup(K(9)) == (sentinel, False)


# ── manager: bulk touch codec + raw-key lookup fast path, combined ──


def test_combined_w1_mixed_states_raw_map_matches_reference():
    # W=1 bulk fill + raw fill: raw entries are the u64-map values, the u64 map
    # matches the old per-key fill in order, lookups match the reference and
    # never issue a Check.
    keys = [K(n) for n in (1, 2, 3, 2, 4, 5, 1, 6)]
    int_keys = [U(n) for n in (1, 2, 3, 2, 4, 5, 1, 6)]
    ring, ref_ring = FakeRing(), FakeRing()
    _seed_mixed_states(ring, sorted(set(int_keys)))
    _seed_mixed_states(ref_ring, sorted(set(int_keys)))
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ref = _ReferenceManager(ref_ring, block_size_bytes=4096)

    expected = {}
    _old_touch_fill(expected, int_keys, 1, ring)

    mgr.touch(keys)
    ref.touch(keys)
    for k in keys:
        assert mgr._raw_lookup_cache[k] is mgr._lookup_cache[_key_to_u64(k)]
    assert list(mgr._lookup_cache.items()) == list(expected.items())
    for k in keys:
        assert mgr.lookup(k) == ref.lookup(k)
    assert _calls_of(ring, "check_states") == []


def test_combined_w1_short_reply_uncovered_keys_false_in_both_maps():
    class ShortRing(FakeRing):
        def touch_states(self, keys, promote=False):
            return super().touch_states(keys, promote)[:2]

    ring = ShortRing()
    for n in (1, 2, 3, 4):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), K(2), K(3), K(4)])
    for n in (3, 4):
        assert mgr._lookup_cache[U(n)] is False
        assert mgr._raw_lookup_cache[K(n)] is False
        assert mgr.lookup(K(n)) is False
    assert mgr.lookup(K(1)) is True
    assert _calls_of(ring, "check_states") == []


def test_combined_w1_lookup_then_touch_clears_both_maps():
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(2)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), K(2)])
    assert mgr.lookup(K(1)) is True
    mgr.touch([K(3)])  # new pass: prior pass's entries are retired
    assert list(mgr._lookup_cache.items()) == [(U(3), False)]
    assert list(mgr._raw_lookup_cache.items()) == [(K(3), False)]
    ring.exists[U(1)] = False
    assert mgr.lookup(K(1)) is False  # authoritative fallback, not stale True
    assert _calls_of(ring, "check_states") == [[U(1)]]


def test_combined_w1_touch_states_sends_int_keys_with_raw_map():
    ring = FakeRing()
    keys = [K(n) for n in (1, 2, 3, 1)]
    int_keys = [U(n) for n in (1, 2, 3, 1)]
    ring.exists[U(2)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch(keys)
    ((sent, promote),) = _calls_of(ring, "touch")
    assert sent == int_keys
    assert promote is False
    assert set(mgr._raw_lookup_cache) == set(keys)


def test_combined_unhashable_key_clears_stale_raw_entry_in_same_pass():
    # touch([K1]) leaves a raw K1=True. With no lookup between, K1 goes missing
    # and touch([bytearray(K2), K1]) aborts the raw update at the bytearray; the
    # old raw K1=True must not survive.
    ring = FakeRing()
    ring.exists[U(1)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1)])
    assert mgr._raw_lookup_cache[K(1)] is True
    ring.exists[U(1)] = False
    mgr.touch([bytearray(K(2)), K(1)])
    assert mgr._raw_lookup_cache == {}
    assert mgr.lookup(K(1)) is False
    assert _calls_of(ring, "check_states") == []


def test_presence_w1_touch_skips_ns_all(monkeypatch):
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)

    def _no_ns_all(_k):
        raise AssertionError("W==1 touch() must not expand keys")

    monkeypatch.setattr(mgr, "_ns_all", _no_ns_all)
    keys = [K(n) for n in range(64)]
    for n in range(0, 64, 2):
        ring.exists[U(n)] = True
    mgr.touch(keys)
    assert len(mgr._lookup_cache) == 64
    assert all(mgr._lookup_cache[U(n)] is (n % 2 == 0) for n in range(64))


def test_presence_lookup_hit_makes_no_import(monkeypatch):
    import builtins

    import certus_shmq_connector.compat as compat
    import certus_shmq_connector.manager as manager

    assert manager._compat is compat
    ring = FakeRing()
    ring.exists[U(1)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1)])
    # A trivial shim, so compat's lazy base-attr import cannot run either.
    monkeypatch.setattr(compat, "lookup_result", lambda hit: hit)

    imports = []
    real_import = builtins.__import__

    def _counting_import(*args, **kwargs):
        imports.append(args[0] if args else kwargs.get("name"))
        return real_import(*args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", _counting_import)
    result = mgr.lookup(K(1))
    seen = list(imports)  # before the restore, which imports on its own
    monkeypatch.setattr(builtins, "__import__", real_import)
    assert result is True
    assert seen == []


# ── manager: prepare_store ──


def test_prepare_store_filters_existing_and_reserves():
    ring = FakeRing()
    ring.exists[U(1)] = True  # already cached -> filtered out
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=8192)
    keys = [K(1), K(2)]
    out = mgr.prepare_store(keys)
    assert out is not None
    assert out.keys_to_store == [keys[1]]
    (entries,) = _calls_of(ring, "reserve")
    assert [e[0] for e in entries] == [U(2)]
    assert [e[1] for e in entries] == [8192]  # size == block_size_bytes


def test_prepare_store_all_existing_is_noop():
    ring = FakeRing()
    ring.exists[U(1)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    out = mgr.prepare_store([K(1)])
    assert out is not None
    assert out.keys_to_store == []
    assert _calls_of(ring, "reserve") == []


def test_prepare_store_skips_key_with_store_in_flight():
    # A pending key is already being written by another in-flight store; store
    # dedup (via the bool check(), where pending counts as present) must not
    # re-reserve it — only the genuinely-absent key is offered for storage.
    from certus_shmq_connector.ring import CHECK_PENDING

    ring = FakeRing()
    ring.states[U(2)] = CHECK_PENDING  # key 2 store in flight
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [K(2), K(3)]
    out = mgr.prepare_store(keys)
    assert out is not None
    assert out.keys_to_store == [keys[1]]  # only key 3
    (entries,) = _calls_of(ring, "reserve")
    assert [e[0] for e in entries] == [U(3)]


def test_prepare_store_partial_reserve_keeps_reserved_drops_failed():
    # Best-effort: reserve is per-key independent, so a partial failure stores
    # the keys that fit and drops the rest (rather than rejecting the whole
    # request, which triggers a vLLM retry+warning storm).
    ring = FakeRing()
    ring.reserve_fail = {U(3)}  # key 3 fails to reserve; key 2 succeeds
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [K(2), K(3)]
    out = mgr.prepare_store(keys)
    assert out is not None
    # Only the reserved key is offered for storage, in offload order.
    assert out.keys_to_store == [keys[0]]
    assert out.store_spec.keys == [U(2)]
    # The reserved key is kept (to be committed later), so no rollback; the
    # failed key allocated nothing, so it needs no abort either.
    assert _calls_of(ring, "abort_store") == []


def test_prepare_store_all_reserve_fail_returns_empty_not_none():
    # When nothing fits, return an empty (non-None) result so vLLM advances past
    # these tokens quietly instead of retrying and warning every scheduler step.
    ring = FakeRing()
    ring.reserve_fail = {U(2), U(3)}
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [K(2), K(3)]
    out = mgr.prepare_store(keys)
    assert out is not None
    assert out.keys_to_store == []
    assert out.store_spec.keys == []
    assert _calls_of(ring, "abort_store") == []


def test_prepare_store_preserves_offload_order_in_partial():
    # store_spec must stay in offload order for the scheduler's positional zip
    # of src GPU block ids with dst keys to line up on a partial subset.
    ring = FakeRing()
    ring.reserve_fail = {U(20)}  # drop the middle key
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [K(10), K(20), K(30)]
    out = mgr.prepare_store(keys)
    assert out.keys_to_store == [keys[0], keys[2]]
    assert out.store_spec.keys == [U(10), U(30)]


# ── manager: complete_store / load ──


def test_complete_store_success_commits():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.complete_store([(9).to_bytes(8, "big")], success=True)
    assert _calls_of(ring, "commit_store")
    assert not _calls_of(ring, "abort_store")


def test_complete_store_failure_aborts():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.complete_store([(9).to_bytes(8, "big")], success=False)
    assert _calls_of(ring, "abort_store")
    assert not _calls_of(ring, "commit_store")


def test_prepare_load_pins_no_promote_and_complete_load_unpins():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    spec = mgr.prepare_load([K(4), K(5)])
    assert isinstance(spec, CertusLoadStoreSpec)
    assert spec.keys == [U(4), U(5)]
    (args,) = _calls_of(ring, "pin")
    keys, promote = args
    assert keys == [U(4), U(5)]
    # promote must be False: Lookup promotes cold entries itself; a Pin-promote
    # would race the Lookup-promote on mt.insert (AlreadyExists -> load crash).
    assert promote is False
    mgr.complete_load([K(4), K(5)])
    (unpin,) = _calls_of(ring, "unpin")
    assert unpin == [U(4), U(5)]


# ── manager: take_events ──


def test_take_events_surfaces_removed_not_demoted():
    ring = FakeRing()
    ring.events = [
        (100, REASON_REMOVED),
        (200, REASON_DEMOTED),
    ]
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    events = list(mgr.take_events())
    assert len(events) == 1
    assert events[0].removed is True
    assert events[0].keys == [(100).to_bytes(8, "big")]
    # second call drains the buffer
    assert list(mgr.take_events()) == []


# ── handler offset wiring ──


def test_store_handler_sends_offsets_per_block():
    from certus_shmq_connector.handler import worker_class
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    ring = FakeRing()
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=1, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)
    # One worker serves both directions; transfer_async routes a store by the
    # source spec being a GPULoadStoreSpec (≤0.24 medium-pair entrypoint). The
    # worker holds a LIST of KV regions (N==1 here — single-tensor block).
    h = worker_class()(ring, [kv], block_size_bytes=1024, executor=executor)

    src = GPULoadStoreSpec(block_ids=[3, 7], group_sizes=[2], block_indices=[0])
    dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
    assert h.transfer_async(job_id=1, spec=(src, dst)) is True
    h.wait({1})
    results = h.get_finished()
    assert len(results) == 1 and results[0].success

    (entries,) = _calls_of(ring, "copy_to_store")
    assert [key for key, _ in entries] == [30, 70]
    # Single-region (N==1): each entry carries one region tuple
    # (handle_bytes, gpu_device_id, offset, size).
    assert all(len(regions) == 1 for _, regions in entries)
    assert [regions[0][2] for _, regions in entries] == [3 * 1024, 7 * 1024]
    assert all(regions[0][0] == b"z" * 64 for _, regions in entries)
    assert all(regions[0][1] == 1 for _, regions in entries)
    executor.shutdown()


def test_store_handler_never_reports_failure_and_aborts_failed_keys():
    """Regression: a failed CopyToStore must NOT surface success=False (vLLM's
    offloading worker asserts transfer_result.success and crashes the engine).
    The failed keys are rolled back via abort_store; the job reports success."""
    from certus_shmq_connector.handler import worker_class
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    ring = FakeRing()
    ring.copy_fail = {70}  # one of two blocks fails to copy
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=0, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)
    h = worker_class()(ring, [kv], block_size_bytes=1024, executor=executor)

    src = GPULoadStoreSpec(block_ids=[3, 7], group_sizes=[2], block_indices=[0])
    dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
    h.transfer_async(job_id=9, spec=(src, dst))
    h.wait({9})
    results = h.get_finished()
    assert len(results) == 1 and results[0].success is True  # never False
    (abort,) = _calls_of(ring, "abort_store")
    assert abort == [70]  # only the failed key rolled back
    executor.shutdown()

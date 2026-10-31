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

import certus_shmq_connector.manager as manager
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

    @staticmethod
    def _expand_blocks(table, keys, block_ids):
        """Expand a layer table + block ids into legacy ``(key, regions)``
        entries, as the server's decode_block_batch does, so offset assertions
        written against the per-region form still hold."""
        import struct

        off = 0
        (n_handles,) = struct.unpack_from("<I", table, off)
        off += 4
        handles = []
        for _ in range(n_handles):
            hb = table[off:off + 64]
            (dev,) = struct.unpack_from("<i", table, off + 64)
            handles.append((hb, dev))
            off += 68
        (n_layers,) = struct.unpack_from("<H", table, off)
        off += 2
        layers = [struct.unpack_from("<IQQI", table, off + 24 * i) for i in range(n_layers)]
        assert off + 24 * n_layers == len(table)
        return [
            (k, [(*handles[idx], base + int(b) * stride, size)
                 for idx, base, stride, size in layers])
            for k, b in zip(keys, block_ids)
        ]

    @staticmethod
    def _expand_chunks(table, scratch_idx, blocks_per_key, keys, block_ids):
        """Expand a ChunkBlockBatch into ``(key, regions)`` entries, as the server's
        decode_chunk_block_batch does: each key's blocks in order, each block's
        regions in layer order; the j-th CHUNK_SCRATCH maps layer L to the scratch
        handle at j * sum(sizes) + sum(sizes[:L]) and CHUNK_ABSENT adds nothing."""
        import struct
        from certus_shmq_connector.ring import CHUNK_ABSENT, CHUNK_SCRATCH

        off = 0
        (n_handles,) = struct.unpack_from("<I", table, off)
        off += 4
        handles = []
        for _ in range(n_handles):
            handles.append((table[off:off + 64], struct.unpack_from("<i", table, off + 64)[0]))
            off += 68
        (n_layers,) = struct.unpack_from("<H", table, off)
        off += 2
        layers = [struct.unpack_from("<IQQI", table, off + 24 * i) for i in range(n_layers)]
        assert off + 24 * n_layers == len(table)
        entries = []
        block_bytes = sum(layer[3] for layer in layers)
        for i, k in enumerate(keys):
            regions = []
            skipped = 0
            for b in block_ids[i * blocks_per_key:(i + 1) * blocks_per_key]:
                if b == CHUNK_ABSENT:
                    continue
                scratch_off = skipped * block_bytes
                if b == CHUNK_SCRATCH:
                    skipped += 1
                for idx, base, stride, size in layers:
                    if b == CHUNK_SCRATCH:
                        regions.append((*handles[scratch_idx], scratch_off, size))
                        scratch_off += size
                    else:
                        regions.append((*handles[idx], base + int(b) * stride, size))
            entries.append((k, regions))
        return entries

    def copy_to_store_chunks(self, table, scratch_idx, blocks_per_key, keys, block_ids,
                             timings=None):
        entries = self._expand_chunks(table, scratch_idx, blocks_per_key, list(keys),
                                      list(block_ids))
        # Recorded under the per-region name: the chunk tests assert the regions a
        # store sends, whichever op carries them.
        self.calls.append(("copy_to_store", entries))
        self.calls.append(("copy_to_store_chunks", entries))
        return [e[0] not in self.copy_fail for e in entries]

    def lookup_chunks(self, table, scratch_idx, blocks_per_key, keys, block_ids, timings=None):
        entries = self._expand_chunks(table, scratch_idx, blocks_per_key, list(keys),
                                      list(block_ids))
        self.calls.append(("lookup", entries))
        self.calls.append(("lookup_chunks", entries))
        return [True] * len(entries)

    def copy_to_store_blocks(self, table, keys, block_ids, timings=None):
        entries = self._expand_blocks(table, list(keys), list(block_ids))
        self.calls.append(("copy_to_store_blocks", entries))
        return [e[0] not in self.copy_fail for e in entries]

    def lookup_blocks(self, table, keys, block_ids, timings=None):
        entries = self._expand_blocks(table, list(keys), list(block_ids))
        self.calls.append(("lookup_blocks", entries))
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


class _FakeClock:
    """Stands in for manager._monotonic_ns; advanced explicitly, in us."""

    def __init__(self, start_ns: int = 10**12):
        self.now_ns = start_ns

    def __call__(self) -> int:
        return self.now_ns

    def advance_us(self, us: int) -> None:
        self.now_ns += us * 1000


@pytest.fixture(autouse=True)
def clock(monkeypatch):
    # Every test runs on a frozen clock with the default TTL, so a slow machine
    # or a CERTUS_LOOKUP_CACHE_TTL_US in the environment cannot expire the
    # lookup cache between a test's touch() and lookup(). Tests that exercise
    # the age limit advance it explicitly.
    c = _FakeClock()
    monkeypatch.setattr(manager, "_monotonic_ns", c)
    monkeypatch.delenv("CERTUS_LOOKUP_CACHE_TTL_US", raising=False)
    return c


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
    # carries per-key residency — so touch() issues NO separate check, and
    # per-key lookups that follow within the age limit (in vLLM >= 0.21: the
    # next request's prefix lookup) are served from the kept answers — no
    # per-key check RPC either.
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
    # A lookup for a key no recent touch covered must consult the
    # authoritative single-key check rather than answering absent from a stale
    # or empty map.
    ring = FakeRing()
    ring.exists[U(42)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1)])  # map covers key 1 only
    assert _calls_of(ring, "check_states") == []  # residency came from touch
    assert mgr.lookup(K(42)) is True
    # The fallback single-key check happened for the uncached key only.
    assert _calls_of(ring, "check_states") == [[U(42)]]


def test_touch_after_lookup_replaces_previous_answers():
    # A touch that follows a lookup replaces the previous touch's answers: the
    # old positive must not survive (the key may since have been evicted), so
    # the next lookup reads the fresh TouchCheck reply.
    ring = FakeRing()
    ring.exists[U(5)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)

    mgr.touch([K(5)])
    assert mgr.lookup(K(5)) is True

    # Key 5 evicted in between; the new TouchCheck reply reflects it.
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
    # The PENDING state must survive the touch()-filled lookup cache: a PENDING
    # key looked up soon after touch is answered from the cached state (no RPC)
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
    # the first-insertion position) and a pre-existing entry from an earlier touch.
    ring = FakeRing()
    keys = [K(n) for n in (1, 2, 3, 2, 4, 5, 1, 6)]
    int_keys = [U(n) for n in (1, 2, 3, 2, 4, 5, 1, 6)]
    _seed_mixed_states(ring, sorted(set(int_keys)))

    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    # Entries from an earlier touch moments ago (still within the age limit).
    mgr._lookup_cache_filled_at_ns = manager._monotonic_ns()
    mgr._lookup_cache[U(3)] = True  # earlier entry, overwritten in place
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
    mgr._lookup_cache_filled_at_ns = manager._monotonic_ns()  # fresh earlier entry
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


def test_touch_w1_lookup_then_touch_still_clears_maps():
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(2)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1), K(2)])
    assert mgr.lookup(K(1)) is True
    mgr.touch([K(3)])  # touch after lookup: earlier entries are retired
    assert list(mgr._lookup_cache.items()) == [(U(3), False)]
    ring.exists[U(1)] = False
    assert mgr.lookup(K(1)) is False  # authoritative fallback, not stale True
    assert _calls_of(ring, "check_states") == [[U(1)]]


# ── manager: lookup cache age limit and vLLM's lookup -> touch order ──
# (the autouse ``clock`` fixture above freezes manager._monotonic_ns)


def test_lookup_cache_ttl_env_default_custom_and_invalid(monkeypatch):
    mgr = ShmqCertusOffloadingManager(FakeRing(), block_size_bytes=4096)
    assert mgr._lookup_cache_ttl_ns == 1000 * 1000  # default 1000 us
    monkeypatch.setenv("CERTUS_LOOKUP_CACHE_TTL_US", "250")
    mgr = ShmqCertusOffloadingManager(FakeRing(), block_size_bytes=4096)
    assert mgr._lookup_cache_ttl_ns == 250 * 1000
    monkeypatch.setenv("CERTUS_LOOKUP_CACHE_TTL_US", "-5")
    mgr = ShmqCertusOffloadingManager(FakeRing(), block_size_bytes=4096)
    assert mgr._lookup_cache_ttl_ns == 0
    monkeypatch.setenv("CERTUS_LOOKUP_CACHE_TTL_US", "1ms")
    with pytest.raises(ValueError, match="CERTUS_LOOKUP_CACHE_TTL_US"):
        ShmqCertusOffloadingManager(FakeRing(), block_size_bytes=4096)


def test_lookup_cache_ttl_read_once_at_construction(monkeypatch, clock):
    ring = FakeRing()
    ring.exists[U(1)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    monkeypatch.setenv("CERTUS_LOOKUP_CACHE_TTL_US", "0")  # too late to matter
    mgr.touch([K(1)])
    assert mgr.lookup(K(1)) is True
    assert _calls_of(ring, "check_states") == []


@pytest.mark.parametrize("w", [1, 2])
def test_lookup_cache_fresh_entry_used_up_to_ttl(clock, w):
    from certus_shmq_connector.mediums import ns_key

    ring = FakeRing()
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=w)
    mgr.touch([K(1), K(2)])
    clock.advance_us(1000)  # exactly the TTL old: still used
    assert mgr.lookup(K(1)) is True  # raw-key path
    assert mgr.lookup(bytearray(K(1))) is True  # fold path (unhashable key)
    assert mgr.lookup(K(2)) is False  # cached miss, also no RPC
    assert _calls_of(ring, "check_states") == []


@pytest.mark.parametrize("w", [1, 2])
def test_lookup_cache_expired_entry_not_used(clock, w):
    # A positive older than the TTL must not be reused: the key may have been
    # evicted since, and a stale hit makes vLLM schedule a load whose Pin fails.
    from certus_shmq_connector.mediums import ns_key

    ring = FakeRing()
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=w)
    mgr.touch([K(1)])
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = False  # evicted after the touch
    clock.advance_us(1001)
    assert mgr.lookup(K(1)) is False  # raw-key map skipped
    assert mgr.lookup(bytearray(K(1))) is False  # u64 map skipped too
    shards = [ns_key(U(1), r, w) for r in range(w)]
    assert _calls_of(ring, "check_states") == [shards, shards]


@pytest.mark.parametrize("w", [1, 2])
def test_lookup_cache_ttl_zero_disables_cache(monkeypatch, clock, w):
    from certus_shmq_connector.mediums import ns_key

    monkeypatch.setenv("CERTUS_LOOKUP_CACHE_TTL_US", "0")
    ring = FakeRing()
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=w)
    mgr.touch([K(1), K(2)])
    # The TouchCheck still goes out over every shard (LRU recency) ...
    shards = [ns_key(U(k), r, w) for k in (1, 2) for r in range(w)]
    assert _calls_of(ring, "touch") == [(shards, False)]
    # ... but nothing is kept, and every lookup is an authoritative Check.
    assert mgr._lookup_cache == {} and mgr._raw_lookup_cache == {}
    assert mgr.lookup(K(1)) is True
    assert mgr.lookup(K(2)) is False
    assert len(_calls_of(ring, "check_states")) == 2


def test_lookup_first_order_next_request_reads_previous_touch(clock):
    # vLLM >= 0.21 order (scheduler.py::get_num_new_matched_tokens): each
    # request's prefix lookup runs BEFORE its touch. Request A's lookups find
    # nothing cached and Check; A's touch keeps the answers; request B, handled
    # right after A and sharing A's prefix, reads them without a Check.
    ring = FakeRing()
    for n in (1, 2):  # shared prefix, resident
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)

    # Request A: K1 K2 shared, K3 private (miss -> the prefix scan stops).
    assert [mgr.lookup(K(n)) for n in (1, 2, 3)] == [True, True, False]
    assert _calls_of(ring, "check_states") == [[U(1)], [U(2)], [U(3)]]
    mgr.touch([K(1), K(2), K(3)])

    clock.advance_us(100)
    # Request B: K1 K2 shared (cached), K4 private (Check).
    ring.calls.clear()
    assert [mgr.lookup(K(n)) for n in (1, 2, 4)] == [True, True, False]
    assert _calls_of(ring, "check_states") == [[U(4)]]
    mgr.touch([K(1), K(2), K(4)])
    # B's touch replaced A's answers (K3 gone, K4 in).
    assert set(mgr._lookup_cache) == {U(1), U(2), U(4)}

    # Next scheduler step, well past the TTL: nothing is reused.
    clock.advance_us(5000)
    ring.calls.clear()
    assert mgr.lookup(K(1)) is True
    assert _calls_of(ring, "check_states") == [[U(1)]]


def test_consecutive_touches_keep_oldest_fill_time(clock):
    # Several touches with no lookup between them (e.g. one per KV group) share
    # one fill time: the OLDEST. An entry is never treated as younger than it
    # is, even when that expires a newer entry early.
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(2)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1)])
    clock.advance_us(600)
    mgr.touch([K(2)])
    clock.advance_us(500)  # K1 is 1100 us old, K2 500 us
    ring.exists[U(1)] = False
    assert mgr.lookup(K(1)) is False  # not the stale True
    assert mgr.lookup(K(2)) is True
    assert _calls_of(ring, "check_states") == [[U(1)], [U(2)]]


def test_touch_after_expiry_restarts_fill_time(clock):
    # A touch that finds the maps expired drops them and starts a new fill
    # time, even with no lookup in between.
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(2)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1)])
    clock.advance_us(2000)
    mgr.touch([K(2)])
    assert set(mgr._lookup_cache) == {U(2)}
    assert set(mgr._raw_lookup_cache) == {K(2)}
    clock.advance_us(900)
    assert mgr.lookup(K(2)) is True
    assert _calls_of(ring, "check_states") == []


def test_touch_into_empty_maps_restarts_fill_time(clock):
    # Maps dropped mid-window (here by prepare_store) are refilled with a new
    # fill time, so the new entries get their full TTL.
    ring = FakeRing()
    ring.exists[U(1)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(9)])
    clock.advance_us(800)
    mgr.prepare_store([K(9)])  # drops the maps
    mgr.touch([K(1)])
    clock.advance_us(900)  # 1700 us after the first touch, 900 after this one
    ring.calls.clear()
    assert mgr.lookup(K(1)) is True
    assert _calls_of(ring, "check_states") == []


@pytest.mark.parametrize("w", [1, 2])
def test_prepare_store_drops_lookup_cache(w):
    # prepare_store's Reserve may make the server evict blocks a fresh cache
    # entry still calls resident; the next lookup must Check, not reuse it.
    from certus_shmq_connector.mediums import ns_key

    ring = FakeRing()
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=w)
    mgr.touch([K(1)])
    mgr.prepare_store([K(7)])
    assert mgr._lookup_cache == {} and mgr._raw_lookup_cache == {}
    for r in range(w):
        ring.exists[ns_key(U(1), r, w)] = False  # evicted by that Reserve
    ring.calls.clear()
    assert mgr.lookup(K(1)) is False
    assert _calls_of(ring, "check_states") == [[ns_key(U(1), r, w) for r in range(w)]]


def test_take_events_removed_drops_lookup_cache():
    ring = FakeRing()
    ring.exists[U(1)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, events_enabled=True)
    mgr._key_reverse[U(1)] = K(1)
    mgr.touch([K(1)])
    ring.events = [(200, REASON_DEMOTED)]  # still loadable: answers are kept
    assert list(mgr.take_events()) == []
    assert U(1) in mgr._lookup_cache
    ring.events = [(U(1), REASON_REMOVED)]
    ring.exists[U(1)] = False
    assert len(list(mgr.take_events())) == 1
    assert mgr._lookup_cache == {} and mgr._raw_lookup_cache == {}
    ring.calls.clear()
    assert mgr.lookup(K(1)) is False
    assert _calls_of(ring, "check_states") == [[U(1)]]


# ── manager: commit-group Check on a lookup miss ──


def _commit(mgr, keys):
    mgr.complete_store(keys, success=True)


def test_commit_group_one_check_answers_whole_group(clock):
    ring = FakeRing()
    for n in range(1, 17):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(n) for n in range(1, 17)])
    ring.calls.clear()
    assert all(mgr.lookup(K(n)) is True for n in range(1, 17))
    assert _calls_of(ring, "check_states") == [[U(n) for n in range(1, 17)]]


def test_commit_group_looked_up_key_first_exactly_once(clock):
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(1), K(2), K(3)])
    ring.calls.clear()
    assert mgr.lookup(K(2)) is False
    assert _calls_of(ring, "check_states") == [[U(2), U(1), U(3)]]


def test_commit_group_pending_sibling_reads_as_miss(clock):
    from certus_shmq_connector.ring import CHECK_PENDING

    ring = FakeRing()
    for n in range(1, 5):
        ring.exists[U(n)] = True
    ring.states[U(2)] = CHECK_PENDING
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(n) for n in range(1, 5)])
    ring.calls.clear()
    assert mgr.lookup(K(1)) is True
    assert mgr.lookup(K(2)) is False
    assert mgr.lookup(K(3)) is True
    assert len(_calls_of(ring, "check_states")) == 1


def test_commit_group_w2_expands_shards_and_ands_ranks(clock):
    from certus_shmq_connector.mediums import ns_key

    w = 2
    ring = FakeRing()
    for n in (1, 2, 3):
        for r in range(w):
            ring.exists[ns_key(U(n), r, w)] = True
    ring.exists[ns_key(U(3), 1, w)] = False  # one shard of K3 missing
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=w)
    _commit(mgr, [K(1), K(2), K(3)])
    ring.calls.clear()
    assert mgr.lookup(K(1)) is True
    assert mgr.lookup(K(2)) is True
    assert mgr.lookup(K(3)) is False
    assert _calls_of(ring, "check_states") == [
        [ns_key(U(n), r, w) for n in (1, 2, 3) for r in range(w)]
    ]


def test_commit_group_answers_expire_after_ttl(clock):
    ring = FakeRing()
    for n in (1, 2, 3):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(1), K(2), K(3)])
    ring.calls.clear()
    assert mgr.lookup(K(1)) is True
    ring.exists[U(2)] = False  # evicted after the group Check
    clock.advance_us(1000)  # exactly the TTL old: still used
    assert mgr.lookup(K(3)) is True
    clock.advance_us(1)
    assert mgr.lookup(K(2)) is False  # not the stale True
    assert _calls_of(ring, "check_states") == [
        [U(1), U(2), U(3)],
        [U(2), U(1), U(3)],
    ]


def test_commit_group_fresh_maps_keep_older_fill_time(clock):
    # A group Check into fresh touch-filled maps keeps their older fill time.
    ring = FakeRing()
    for n in (1, 2, 9):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(1), K(2)])
    mgr.touch([K(9)])
    filled_at = mgr._lookup_cache_filled_at_ns
    clock.advance_us(600)
    assert mgr.lookup(K(1)) is True
    assert mgr._lookup_cache_filled_at_ns == filled_at
    clock.advance_us(500)  # 1100 us after the touch: everything expired
    ring.calls.clear()
    assert mgr.lookup(K(2)) is True
    assert _calls_of(ring, "check_states") == [[U(2), U(1)]]
    assert mgr._lookup_cache_filled_at_ns == clock.now_ns


def test_commit_group_answers_dropped_by_prepare_store_and_removed(clock):
    ring = FakeRing()
    for n in (1, 2, 3):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, events_enabled=True)
    _commit(mgr, [K(1), K(2), K(3)])
    assert mgr.lookup(K(1)) is True
    mgr.prepare_store([K(7)])
    assert mgr._lookup_cache == {} and mgr._raw_lookup_cache == {}
    ring.calls.clear()
    assert mgr.lookup(K(2)) is True
    assert _calls_of(ring, "check_states") == [[U(2), U(1), U(3)]]

    ring.events = [(U(3), REASON_REMOVED)]
    ring.exists[U(3)] = False
    list(mgr.take_events())
    assert mgr._lookup_cache == {} and mgr._raw_lookup_cache == {}
    ring.calls.clear()
    assert mgr.lookup(K(3)) is False
    assert _calls_of(ring, "check_states") == [[U(3), U(1), U(2)]]


def test_commit_group_answers_replaced_by_following_touch(clock):
    ring = FakeRing()
    for n in (1, 2):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(1), K(2)])
    assert mgr.lookup(K(1)) is True
    mgr.touch([K(5)])
    assert set(mgr._lookup_cache) == {U(5)}
    assert set(mgr._raw_lookup_cache) == {K(5)}


def test_commit_group_ttl_zero_stays_single_key(monkeypatch, clock):
    monkeypatch.setenv("CERTUS_LOOKUP_CACHE_TTL_US", "0")
    ring = FakeRing()
    for n in (1, 2, 3):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(1), K(2), K(3)])
    ring.calls.clear()
    assert [mgr.lookup(K(n)) for n in (1, 2, 3)] == [True, True, True]
    assert _calls_of(ring, "check_states") == [[U(1)], [U(2)], [U(3)]]
    assert mgr._lookup_cache == {} and mgr._raw_lookup_cache == {}


def test_commit_group_absent_or_single_key_commit_is_single_check(clock):
    ring = FakeRing()
    ring.exists[U(5)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(5)])
    assert len(mgr._commit_groups) == 0
    ring.calls.clear()
    assert mgr.lookup(K(5)) is True
    assert mgr.lookup(K(50)) is False
    assert _calls_of(ring, "check_states") == [[U(5)], [U(50)]]
    assert mgr._lookup_cache == {} and mgr._raw_lookup_cache == {}


def test_commit_group_failed_store_records_nothing_and_drops_record(clock):
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.complete_store([K(7), K(8)], success=False)
    assert len(mgr._commit_groups) == 0
    _commit(mgr, [K(n) for n in range(1, 5)])
    mgr.complete_store([K(1), K(2)], success=False)
    assert set(mgr._commit_groups) == {U(3), U(4)}
    ring.calls.clear()
    mgr.lookup(K(1))
    assert _calls_of(ring, "check_states") == [[U(1)]]


def test_commit_group_duplicate_key_not_repeated_in_check(clock):
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(1), K(2), K(1), K(3), K(2)])
    assert len(mgr._commit_groups[U(1)]) == 3
    ring.calls.clear()
    mgr.lookup(K(3))
    assert _calls_of(ring, "check_states") == [[U(3), U(1), U(2)]]


def test_commit_group_record_bounded_and_split(monkeypatch, clock):
    monkeypatch.setattr(manager, "_COMMIT_GROUP_MAX", 4)
    monkeypatch.setattr(manager, "_COMMIT_GROUP_KEYS_MAX", 6)
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    # Chunks [1-4] [5-8] [9 10]; the 4 oldest keys are dropped to stay at 6.
    _commit(mgr, [K(n) for n in range(1, 11)])
    assert list(mgr._commit_groups) == [U(n) for n in range(5, 11)]
    assert [k for _, k in mgr._commit_groups[U(6)]] == [U(n) for n in range(5, 9)]
    assert mgr._commit_groups[U(5)] is mgr._commit_groups[U(8)]
    assert [k for _, k in mgr._commit_groups[U(10)]] == [U(9), U(10)]
    ring.calls.clear()
    mgr.lookup(K(1))
    assert _calls_of(ring, "check_states") == [[U(1)]]


def test_lookup_check_log_written_only_when_artifacts_dir_set(
    monkeypatch, tmp_path, clock
):
    import json
    import os

    monkeypatch.delenv("OPTIMIZER_ARTIFACTS_DIR", raising=False)
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    assert mgr._check_log is None
    _commit(mgr, [K(1), K(2)])
    mgr.lookup(K(1))
    mgr.lookup(K(2))
    assert list(tmp_path.iterdir()) == []

    monkeypatch.setenv("OPTIMIZER_ARTIFACTS_DIR", str(tmp_path))
    for n in range(1, 5):
        ring.exists[U(n)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    _commit(mgr, [K(n) for n in range(1, 5)])
    assert [mgr.lookup(K(n)) for n in range(1, 5)] == [True] * 4
    mgr.lookup(K(50))
    mgr._check_log.flush()
    path = tmp_path / f"lookup_checks.{os.getpid()}.jsonl"
    lines = [json.loads(l) for l in path.read_text().splitlines()]
    assert [(d["keys"], d["group"], d["hits_before"]) for d in lines] == [
        (4, True, 0),
        (1, False, 3),
    ]
    assert lines[0]["t_us"] == 0
    assert all(d["dur_us"] >= 0 for d in lines)


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

    # Key 5 evicted in between; the touch after the lookup drops every prior
    # entry.
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
    mgr.touch([K(3)])  # touch after lookup: earlier entries are retired
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


def test_combined_unhashable_key_clears_stale_raw_entry_between_touches():
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


def _store_keys(mgr, ring, keys):
    out = mgr.prepare_store(keys)
    assert out is not None
    return out


def test_take_events_surfaces_removed_not_demoted():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, events_enabled=True)
    key = b"\x01" * 36
    _store_keys(mgr, ring, [key])
    from certus_shmq_connector.manager import _key_to_u64

    k = _key_to_u64(key)
    ring.events = [(k, REASON_REMOVED), (200, REASON_DEMOTED)]
    events = list(mgr.take_events())
    assert len(events) == 1
    assert events[0].removed is True
    # real OffloadKey round-trips, not the folded u64
    assert events[0].keys == [key]
    assert events[0].medium == "STORAGE"
    # second call drains the buffer
    assert list(mgr.take_events()) == []


def test_take_events_medium_matches_vllm_tracker_enum_keys():
    import importlib

    import conftest
    from certus_shmq_connector import compat

    conftest.build_fake_vllm((0, 29))
    importlib.reload(compat)
    try:
        from vllm.v1.kv_offload.base import Medium

        ring = FakeRing()
        mgr = ShmqCertusOffloadingManager(
            ring, block_size_bytes=4096, events_enabled=True
        )
        key = b"\x03" * 36
        _store_keys(mgr, ring, [key])
        ring.events = [(_key_to_u64(key), REASON_REMOVED)]
        (event,) = mgr.take_events()

        tracker = {Medium.CPU: "cpu", Medium.STORAGE: "storage"}
        assert tracker[event.medium] == "storage"
    finally:
        conftest.build_fake_vllm((0, 20))
        importlib.reload(compat)


def test_a_committed_store_is_announced_once_with_its_vllm_keys():
    # Peer selection M3: a router learns from KV events that this pod's certus
    # holds a block, so a successful commit must yield a stored event carrying
    # vLLM's own keys (the tracker looks its metadata up by them).
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, events_enabled=True)
    keys = [b"\x05" * 36, b"\x06" * 36]
    _store_keys(mgr, ring, keys)
    mgr.complete_store(iter(keys), success=True)  # an iterator: read once only
    (event,) = mgr.take_events()
    assert (event.removed, event.keys, event.medium) == (False, keys, "STORAGE")
    assert list(mgr.take_events()) == [], "announced once"


def test_a_failed_store_is_not_announced():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, events_enabled=True)
    keys = [b"\x07" * 36]
    _store_keys(mgr, ring, keys)
    mgr.complete_store(keys, success=False)
    assert list(mgr.take_events()) == []


def test_store_events_need_events_enabled():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [b"\x08" * 36]
    _store_keys(mgr, ring, keys)
    mgr.complete_store(keys, success=True)
    assert list(mgr.take_events()) == []


def test_a_store_precedes_its_removal_in_one_drain():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, events_enabled=True)
    key = b"\x09" * 36
    _store_keys(mgr, ring, [key])
    mgr.complete_store([key], success=True)
    ring.events = [(_key_to_u64(key), REASON_REMOVED)]
    assert [e.removed for e in mgr.take_events()] == [False, True]


def test_the_event_medium_bridge(monkeypatch):
    # CERTUS_EVENT_MEDIUM=cpu reports certus as CPU, the only tier llm-d's
    # p2p-source-producer counts as pullable; anything else is refused.
    from certus_shmq_connector import compat

    monkeypatch.setenv("CERTUS_EVENT_MEDIUM", "cpu")
    assert compat.offload_event_medium() == "CPU"
    monkeypatch.setenv("CERTUS_EVENT_MEDIUM", "ssd")
    with pytest.raises(ValueError):
        compat.offload_event_medium()
    monkeypatch.delenv("CERTUS_EVENT_MEDIUM")
    assert compat.offload_event_medium() == "STORAGE"


def test_take_events_unknown_key_skipped():
    ring = FakeRing()
    ring.events = [(100, REASON_REMOVED)]
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, events_enabled=True)
    assert list(mgr.take_events()) == []


def test_take_events_disabled_yields_nothing_but_drains():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    key = b"\x02" * 36
    _store_keys(mgr, ring, [key])
    from certus_shmq_connector.manager import _key_to_u64

    ring.events = [(_key_to_u64(key), REASON_REMOVED)]
    assert list(mgr.take_events()) == []
    assert ring.events == []


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

    assert _calls_of(ring, "copy_to_store") == []  # block-id op, not per-region
    (entries,) = _calls_of(ring, "copy_to_store_blocks")
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


def test_transfer_time_is_the_transfer_not_queue_or_reap_wait(monkeypatch):
    """transfer_time covers the transfer on its pool thread, as cputier's CUDA-event
    time covers its copy: not the wait for a pool thread, nor the time until vLLM
    next calls get_finished."""
    import certus_shmq_connector.handler as handler
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    now = [0.0]
    monkeypatch.setattr(handler.time, "monotonic", lambda: now[0])
    ring = FakeRing()
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=0, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)   # job 2 queues behind job 1
    h = handler.worker_class()(ring, [kv], block_size_bytes=1024, executor=executor)
    copy_to_store = ring.copy_to_store
    copy_to_store_blocks = ring.copy_to_store_blocks

    def slow_copy(entries):
        now[0] += 0.25   # each transfer takes 0.25 s
        return copy_to_store(entries)

    def slow_copy_blocks(*args, **kwargs):
        now[0] += 0.25   # the block-id store op, used when a key holds one GPU block
        return copy_to_store_blocks(*args, **kwargs)

    ring.copy_to_store = slow_copy
    ring.copy_to_store_blocks = slow_copy_blocks
    for job_id in (1, 2):
        src = GPULoadStoreSpec(block_ids=[job_id], group_sizes=[1], block_indices=[0])
        dst = CertusLoadStoreSpec([BlockLocation(key=job_id)])
        h.transfer_async(job_id=job_id, spec=(src, dst))
    h.wait({1, 2})
    now[0] += 10.0       # vLLM's next get_finished comes much later
    results = sorted(h.get_finished(), key=lambda r: r.job_id)
    assert [r.job_id for r in results] == [1, 2]
    assert [r.transfer_time for r in results] == [0.25, 0.25]
    executor.shutdown()


# ── offload blocks larger than the GPU block (blocks_per_chunk > 1) ──


def _chunk_worker(ring, blocks_per_chunk, regions=None):
    from certus_shmq_connector.handler import worker_class

    regions = regions or [
        KvCacheIpc(handle_bytes=b"a" * 64, gpu_device_id=0, stride_bytes=100, base_delta=0),
        KvCacheIpc(handle_bytes=b"b" * 64, gpu_device_id=0, stride_bytes=100, base_delta=7),
    ]
    executor = ThreadPoolExecutor(max_workers=1)
    w = worker_class()(ring, regions, block_size_bytes=200, executor=executor,
                       blocks_per_chunk=blocks_per_chunk)
    return w, executor


def _gpu_spec(block_ids, first_index=0):
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
    return GPULoadStoreSpec(block_ids=block_ids, group_sizes=[len(block_ids)],
                            block_indices=[first_index])


def test_chunk_store_puts_each_keys_gpu_blocks_side_by_side():
    ring = FakeRing()
    w, ex = _chunk_worker(ring, blocks_per_chunk=3)
    dst = CertusLoadStoreSpec([BlockLocation(key=10), BlockLocation(key=20)])
    w.submit_store(1, _gpu_spec([5, 6, 7, 8, 9, 4]), dst)
    w.wait({1})
    (entries,) = _calls_of(ring, "copy_to_store")
    assert [k for k, _ in entries] == [10, 20]
    # key 10 = GPU blocks 5,6,7; key 20 = 8,9,4; each block's 2 regions in order.
    want = {10: [5, 6, 7], 20: [8, 9, 4]}
    for key, pieces in entries:
        assert [(h[:1], off) for h, _, off, _ in pieces] == [
            (hb, base + b * 100) for b in want[key] for hb, base in ((b"a", 0), (b"b", 7))
        ]
        assert sum(size for *_, size in pieces) == 3 * 200
    ex.shutdown()


def test_chunk_load_from_inside_a_chunk_sends_skipped_blocks_to_scratch(monkeypatch):
    import certus_shmq_connector.handler as handler

    scratch = KvCacheIpc(handle_bytes=b"s" * 64, gpu_device_id=0, stride_bytes=0, base_delta=0)
    made = []
    monkeypatch.setattr(handler, "scratch_ipc", lambda n, dev: made.append(n) or scratch)
    ring = FakeRing()
    w, ex = _chunk_worker(ring, blocks_per_chunk=4)
    # The request's GPU blocks 6.. (block_indices 6): 6 % 4 = 2 skipped in key 30.
    src = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=40)])
    w.submit_load(2, src, _gpu_spec([11, 12, 13, 14, 15, 16], first_index=6))
    w.wait({2})
    assert all(r.success for r in w.get_finished())
    (entries,) = _calls_of(ring, "lookup")
    k30, k40 = dict(entries)[30], dict(entries)[40]
    # Whole slot covered: 4 blocks x 2 regions, the first 2 blocks into scratch.
    assert len(k30) == 8 and sum(s for *_, s in k30) == 4 * 200
    assert [h[:1] for h, *_ in k30] == [b"s"] * 4 + [b"a", b"b"] * 2
    assert [off for _, _, off, _ in k30[4:]] == [1100, 1107, 1200, 1207]
    assert [off for _, _, off, _ in k40] == [1300, 1307, 1400, 1407, 1500, 1507, 1600, 1607]
    # Each skipped (block, layer) piece at its own scratch bytes: no two overlap.
    assert [off for _, _, off, _ in k30[:4]] == [0, 100, 200, 300]
    assert made == [600]     # room for the 3 blocks a 4-block key can skip
    ex.shutdown()


def test_chunk_load_skipping_nothing_allocates_no_scratch(monkeypatch):
    import certus_shmq_connector.handler as handler

    def no_scratch(n, dev):
        raise AssertionError("scratch allocated for a load that skips nothing")

    monkeypatch.setattr(handler, "scratch_ipc", no_scratch)
    ring = FakeRing()
    w, ex = _chunk_worker(ring, blocks_per_chunk=4)
    src = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=40)])
    w.submit_load(4, src, _gpu_spec([1, 2, 3, 4, 5, 6], first_index=8))
    w.wait({4})
    assert all(r.success for r in w.get_finished())
    ex.shutdown()


def test_chunk_load_last_key_may_be_short():
    ring = FakeRing()
    w, ex = _chunk_worker(ring, blocks_per_chunk=4)
    src = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=40)])
    w.submit_load(3, src, _gpu_spec([1, 2, 3, 4, 5, 6]))
    w.wait({3})
    (entries,) = _calls_of(ring, "lookup")
    assert [len(p) // 2 for _, p in entries] == [4, 2]
    ex.shutdown()


@pytest.mark.parametrize("blocks,keys,first", [(9, 2, 0), (2, 2, 0), (3, 1, 2)])
def test_chunk_block_count_must_match_keys(blocks, keys, first):
    from certus_shmq_connector.handler import _chunk_entries

    regions = [KvCacheIpc(handle_bytes=b"a" * 64, gpu_device_id=0, stride_bytes=1, base_delta=0)]
    with pytest.raises(AssertionError):
        _chunk_entries(regions, list(range(blocks)), list(range(keys)), 4, first, None)


def test_one_block_per_key_is_unchanged():
    # One GPU block per key sends the block-id op; with that op unavailable the worker
    # falls back to per-region entries, which must carry the same offsets as before
    # blocks_per_chunk existed.
    from certus_shmq_connector.handler import worker_class
    ring = FakeRing()
    w, ex = _chunk_worker(ring, blocks_per_chunk=1)
    w._layer_table = None            # this worker always uses the per-region ops
    dst = CertusLoadStoreSpec([BlockLocation(key=1), BlockLocation(key=2)])
    w.submit_store(4, _gpu_spec([3, 7]), dst)
    w.wait({4})
    (entries,) = _calls_of(ring, "copy_to_store")
    assert [[off for _, _, off, _ in p] for _, p in entries] == [[300, 307], [700, 707]]
    ex.shutdown()


def test_several_blocks_per_key_use_the_chunk_ops():
    # A key of several GPU blocks goes over the chunk op, not the one-block-per-key
    # block op and not per-region entries.
    ring = FakeRing()
    w, ex = _chunk_worker(ring, blocks_per_chunk=2)
    dst = CertusLoadStoreSpec([BlockLocation(key=1)])
    w.submit_store(5, _gpu_spec([3, 7]), dst)
    w.wait({5})
    assert not _calls_of(ring, "copy_to_store_blocks")
    (entries,) = _calls_of(ring, "copy_to_store_chunks")
    assert len(entries) == 1 and len(entries[0][1]) == 4      # 2 blocks x 2 regions
    ex.shutdown()


@pytest.mark.parametrize("bpc, n_blocks, n_keys, skip", [
    (4, 8, 2, 0),        # aligned
    (4, 7, 2, 1),        # a load starting 1 block into its first key
    (4, 5, 2, 3),        # ... 3 blocks in
    (4, 6, 2, 0),        # a short last key
    (4, 3, 2, 2),        # skip and a short last key together
    (2, 9, 5, 1),
    (8, 8, 1, 0),
])
def test_chunk_ops_send_the_same_regions_as_the_per_region_form(bpc, n_blocks, n_keys, skip):
    """For every shape of commit 5ce82ee9's mapping, the chunk op's entries (as the
    server expands them) equal what _chunk_entries sends over the per-region op."""
    from certus_shmq_connector.handler import _chunk_entries

    regions = [
        KvCacheIpc(handle_bytes=b"a" * 64, gpu_device_id=0, stride_bytes=100, base_delta=0),
        KvCacheIpc(handle_bytes=b"b" * 64, gpu_device_id=0, stride_bytes=100, base_delta=7),
    ]
    gpu_ids = [50 + 3 * i for i in range(n_blocks)]        # scattered, as vLLM's free list
    keys = [1000 + i for i in range(n_keys)]

    ring = FakeRing()
    w, ex = _chunk_worker(ring, blocks_per_chunk=bpc, regions=regions)
    if skip:
        w.submit_load(1, CertusLoadStoreSpec([BlockLocation(key=k) for k in keys]),
                      _gpu_spec(gpu_ids, first_index=skip))
    else:
        w.submit_store(1, _gpu_spec(gpu_ids), CertusLoadStoreSpec([BlockLocation(key=k) for k in keys]))
    w.wait({1})
    op = "lookup_chunks" if skip else "copy_to_store_chunks"
    (sent,) = _calls_of(ring, op)
    want = _chunk_entries(regions, gpu_ids, keys, bpc, skip, w._scratch)
    assert sent == want
    ex.shutdown()


# ── explicit fetch (T086, contracts/shmq-fetch.md) ──


class FetchRing(FakeRing):
    """A FakeRing whose fetch() puts the started keys in PENDING, as the server
    does; a test then lands or misses them by editing ``states``."""

    def __init__(self, status=0):
        super().__init__()
        self.fetches = []
        self.fetch_status = status

    def fetch(self, peer, entries):
        from certus_shmq_connector.ring import CHECK_MISS, CHECK_PENDING

        entries = list(entries)
        self.fetches.append((peer, entries))
        if self.fetch_status != 0:
            return self.fetch_status, [False] * len(entries)
        started = []
        for key, _ in entries:
            ok = self._state_of(key) == CHECK_MISS
            started.append(ok)
            if ok:
                self.states[key] = CHECK_PENDING
        return 0, started


class Ctx:
    def __init__(self, req_id, peer="10.0.0.105:7481"):
        self.req_id = req_id
        self.kv_transfer_params = {"certus_fetch_from": peer} if peer else None


@pytest.fixture
def lr(monkeypatch):
    """vLLM 0.30's lookup results (MISS/HIT/HIT_PENDING/RETRY): the fake vLLM is
    rebuilt for 0.30 and compat's CAPS follows; both are restored after."""
    import importlib

    import conftest
    from certus_shmq_connector import compat

    monkeypatch.delenv("CERTUS_VLLM_VERSION", raising=False)
    conftest.build_fake_vllm((0, 30))
    monkeypatch.setattr(compat, "CAPS", compat.caps_for((0, 30)))
    yield importlib.import_module("vllm.v1.kv_offload.base").LookupResult
    conftest.build_fake_vllm((0, 20))


def scan(mgr, ctx, keys, lr):
    """vLLM's prefix scan: lookup each key in order, stop at the first MISS.
    Returns the answers given."""
    out = []
    for k in keys:
        r = mgr.lookup(K(k), ctx)
        out.append(r)
        if r == lr.MISS:
            break
    return out


def test_a_hinted_miss_run_is_pending_then_fetched_in_one_op(lr):
    ring = FetchRing()
    ring.exists = {U(1): True}  # 1 local, 2.. are not
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ctx = Ctx("r1")
    # Scan 1: a local hit, then the miss run answered HIT_PENDING so the scan
    # carries on to the end and the run is known.
    assert scan(mgr, ctx, [1, 2, 3, 4], lr) == [lr.HIT] + [lr.HIT_PENDING] * 3
    assert ring.fetches == [], "nothing is sent mid-scan"
    mgr.on_schedule_end(None)
    assert ring.fetches == [("10.0.0.105:7481", [(U(2), 4096), (U(3), 4096), (U(4), 4096)])]
    # Scan 2: in flight.
    assert scan(mgr, ctx, [1, 2, 3, 4], lr) == [lr.HIT] + [lr.HIT_PENDING] * 3
    # The peer has 2 and 3, not 4: the fetch lands 2, 3 and cuts at 4.
    from certus_shmq_connector.ring import CHECK_MISS, CHECK_RESIDENT

    ring.states.update({U(2): CHECK_RESIDENT, U(3): CHECK_RESIDENT, U(4): CHECK_MISS})
    assert scan(mgr, ctx, [1, 2, 3, 4], lr) == [lr.HIT, lr.HIT, lr.HIT, lr.MISS]
    mgr.on_schedule_end(None)
    assert len(ring.fetches) == 1, "a request's run is fetched once"
    s = mgr.fetch_stats()
    assert (s["sent"], s["hits_after_fetch"], s["live"]) == (1, 2, 1)
    mgr.on_request_finished(ctx)
    assert mgr.fetch_stats()["live"] == 0


def test_no_hint_behaves_as_before(lr):
    ring = FetchRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    assert scan(mgr, Ctx("r", peer=None), [1, 2], lr) == [lr.MISS]
    mgr.on_schedule_end(None)
    assert ring.fetches == []


def test_the_budget_ends_the_wait(lr, clock, monkeypatch):
    monkeypatch.setenv("CERTUS_FETCH_BUDGET_MS", "50")
    monkeypatch.setenv("CERTUS_FETCH_BUDGET_US_PER_BLOCK", "0")
    ring = FetchRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ctx = Ctx("r")
    assert scan(mgr, ctx, [1, 2], lr) == [lr.HIT_PENDING] * 2
    mgr.on_schedule_end(None)
    clock.advance_us(51_000)
    # Still pending on the server, but the request waits no longer.
    assert scan(mgr, ctx, [1, 2], lr) == [lr.MISS]
    assert mgr.fetch_stats()["budget_spent"] == 1


def test_the_budget_grows_with_the_run(lr, clock, monkeypatch):
    """A long run gets longer to land: fixed part plus a per-key part, counted
    from the run actually sent. Measured on node5: 367 blocks fetched but none
    served under a flat 200 ms, because the wait ended before they landed."""
    monkeypatch.setenv("CERTUS_FETCH_BUDGET_MS", "50")
    monkeypatch.setenv("CERTUS_FETCH_BUDGET_US_PER_BLOCK", "10000")
    ring = FetchRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ctx = Ctx("r")
    keys = [1, 2, 3, 4]
    assert scan(mgr, ctx, keys, lr) == [lr.HIT_PENDING] * 4
    mgr.on_schedule_end(None)
    # 50 ms + 4 x 10 ms = 90 ms: past the fixed part, the wait goes on.
    clock.advance_us(80_000)
    assert scan(mgr, ctx, keys, lr) == [lr.HIT_PENDING] * 4
    assert mgr.fetch_stats()["budget_spent"] == 0
    clock.advance_us(11_000)
    assert scan(mgr, ctx, keys, lr) == [lr.MISS]
    assert mgr.fetch_stats()["budget_spent"] == 1


def test_only_the_run_sent_extends_the_budget(lr, clock, monkeypatch):
    """Keys the server would not start (already pending elsewhere) are not part
    of the run, so they buy no wait."""
    from certus_shmq_connector.ring import CHECK_PENDING

    monkeypatch.setenv("CERTUS_FETCH_BUDGET_MS", "50")
    monkeypatch.setenv("CERTUS_FETCH_BUDGET_US_PER_BLOCK", "10000")
    ring = FetchRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ctx = Ctx("r")
    assert scan(mgr, ctx, [1, 2, 3, 4], lr) == [lr.HIT_PENDING] * 4
    ring.states[U(2)] = CHECK_PENDING  # someone else's store lands on key 2 first
    mgr.on_schedule_end(None)
    # Run is [1] only: 50 + 10 = 60 ms.
    clock.advance_us(61_000)
    scan(mgr, ctx, [1], lr)
    assert mgr.fetch_stats()["budget_spent"] == 1


def test_a_refused_fetch_computes_instead(lr):
    ring = FetchRing(status=1)  # no endpoint
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ctx = Ctx("r")
    scan(mgr, ctx, [1, 2], lr)
    mgr.on_schedule_end(None)
    assert scan(mgr, ctx, [1, 2], lr) == [lr.MISS]
    assert mgr.fetch_stats()["refused"] == 1
    mgr.on_schedule_end(None)
    assert len(ring.fetches) == 1, "a refusal is not retried"


def test_a_key_already_here_closes_the_run(lr):
    from certus_shmq_connector.ring import CHECK_PENDING

    ring = FetchRing()
    ring.states = {U(3): CHECK_PENDING}  # someone else's store of 3 in flight
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ctx = Ctx("r")
    # 1, 2 gathered; 3 is not loadable (MISS), which ends vLLM's scan there.
    assert scan(mgr, ctx, [1, 2, 3, 4], lr) == [lr.HIT_PENDING, lr.HIT_PENDING, lr.MISS]
    mgr.on_schedule_end(None)
    assert [k for k, _ in ring.fetches[0][1]] == [U(1), U(2)]


def test_tp_fetches_every_shard_and_needs_all_of_them(lr):
    from certus_shmq_connector.mediums import ns_key
    from certus_shmq_connector.ring import CHECK_RESIDENT

    ring = FetchRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096, world_size=2)
    ctx = Ctx("r")
    scan(mgr, ctx, [1, 2], lr)
    mgr.on_schedule_end(None)
    keys = [k for k, _ in ring.fetches[0][1]]
    assert keys == [ns_key(U(1), r, 2) for r in range(2)] + [ns_key(U(2), r, 2) for r in range(2)]
    # One shard of block 1 lands: the block is still pending, not a hit.
    ring.states[ns_key(U(1), 0, 2)] = CHECK_RESIDENT
    assert mgr.lookup(K(1), ctx) == lr.HIT_PENDING


def test_a_closed_run_takes_no_later_keys(lr):
    """A key already here ends the run being gathered: a miss after it must not
    join the run even though vLLM's scan reaches it (the scan continues past a
    HIT), since the fetch's run is the leading misses, in order."""
    ring = FetchRing()
    ring.exists = {U(2): True}  # 1 missing, 2 local, 3 missing
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ctx = Ctx("r")
    assert scan(mgr, ctx, [1, 2, 3], lr) == [lr.HIT_PENDING, lr.HIT, lr.MISS]
    mgr.on_schedule_end(None)
    assert [k for k, _ in ring.fetches[0][1]] == [U(1)]


def test_handler_falls_back_to_region_ops_on_unknown_opcode():
    """A server without the *_BLOCKS opcodes answers "unknown opcode"; the worker
    then sends the same blocks over the legacy per-region ops, with the same
    offsets, and stays on them."""
    from certus_shmq_connector.handler import GPULoadStoreSpec, worker_class
    from certus_shmq_connector.ring import RingError

    class OldServerRing(FakeRing):
        def copy_to_store_blocks(self, table, keys, block_ids, timings=None):
            self.calls.append(("copy_to_store_blocks", None))
            raise RingError("server error (op=20): unknown opcode 20")

        def lookup_blocks(self, table, keys, block_ids, timings=None):
            self.calls.append(("lookup_blocks", None))
            raise RingError("server error (op=19): unknown opcode 19")

    cls = worker_class()
    cls._blocks_supported = True      # a class flag other tests may have cleared
    ring = OldServerRing()
    regions = [
        KvCacheIpc(handle_bytes=b"a" * 64, gpu_device_id=0, stride_bytes=1024, base_delta=0),
        KvCacheIpc(handle_bytes=b"b" * 64, gpu_device_id=0, stride_bytes=1024, base_delta=64),
    ]
    executor = ThreadPoolExecutor(max_workers=1)
    try:
        h = cls(ring, regions, block_size_bytes=2048, executor=executor)
        src = GPULoadStoreSpec(block_ids=[3, 7], group_sizes=[2], block_indices=[0])
        dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
        h.transfer_async(job_id=1, spec=(src, dst))
        h.wait({1})
        assert h.get_finished()[0].success

        (entries,) = _calls_of(ring, "copy_to_store")
        assert [[r[2] for r in regs] for _, regs in entries] == [
            [3 * 1024, 64 + 3 * 1024], [7 * 1024, 64 + 7 * 1024],
        ]
        assert len(_calls_of(ring, "copy_to_store_blocks")) == 1

        # The rejection is latched: a load goes straight to the legacy op.
        load_src = CertusLoadStoreSpec([BlockLocation(key=30)])
        load_dst = GPULoadStoreSpec(block_ids=[3], group_sizes=[1], block_indices=[0])
        h.transfer_async(job_id=2, spec=(load_src, load_dst))
        h.wait({2})
        assert h.get_finished()[0].success
        assert _calls_of(ring, "lookup_blocks") == []
        assert len(_calls_of(ring, "lookup")) == 1
    finally:
        cls._blocks_supported = True
        executor.shutdown()

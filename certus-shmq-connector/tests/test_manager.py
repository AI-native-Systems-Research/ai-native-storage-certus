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
    # Option 1: touch() ships the whole key list, so it fires ONE batched check
    # and the scheduler's subsequent per-key lookup loop is served from the
    # memoized bitmap — no per-key check RPC.
    ring = FakeRing()
    ring.exists[U(1)] = True
    ring.exists[U(2)] = True
    # key 3 absent
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [K(1), K(2), K(3)]

    mgr.touch(keys)
    # Exactly one batched (tri-state) check over the full list.
    checks = _calls_of(ring, "check_states")
    assert checks == [[U(1), U(2), U(3)]]

    # Per-key lookups answer from the cache, issuing NO further check.
    assert mgr.lookup(keys[0]) is True
    assert mgr.lookup(keys[1]) is True
    assert mgr.lookup(keys[2]) is False
    assert _calls_of(ring, "check_states") == [[U(1), U(2), U(3)]]  # still just the one


def test_lookup_miss_falls_back_to_single_check():
    # A lookup for a key the current pass never touched must consult the
    # authoritative single-key check rather than answering absent from a stale
    # or empty bitmap.
    ring = FakeRing()
    ring.exists[U(42)] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([K(1)])  # bitmap covers key 1 only
    assert mgr.lookup(K(42)) is True
    # The fallback single-key check happened for the uncached key.
    assert [U(1)] in _calls_of(ring, "check_states")
    assert [U(42)] in _calls_of(ring, "check_states")


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


def test_lookup_pending_maps_to_none_on_legacy_contract():
    # A store in flight -> Check PENDING. On the ≤0.24 bool|None contract (the
    # conftest default), pending is None ("delay + retry"), never True: the block
    # is coming but not yet loadable. (On 0.26 the shim yields HIT_PENDING.)
    from certus_shmq_connector.ring import CHECK_PENDING

    ring = FakeRing()
    ring.states[U(7)] = CHECK_PENDING
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    assert mgr.lookup(K(7)) is None


def test_touch_caches_pending_state_for_following_lookups():
    # The tri-state must survive the touch()-batched cache: a pending key looked
    # up after touch answers from the cached state (no extra RPC) and still maps
    # to the pending result, not resident.
    from certus_shmq_connector.ring import CHECK_PENDING

    ring = FakeRing()
    ring.exists[U(1)] = True  # resident
    ring.states[U(2)] = CHECK_PENDING  # store in flight
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [K(1), K(2)]

    mgr.touch(keys)
    assert _calls_of(ring, "check_states") == [[U(1), U(2)]]
    assert mgr.lookup(keys[0]) is True  # resident
    assert mgr.lookup(keys[1]) is None  # pending -> legacy None, from cache
    assert _calls_of(ring, "check_states") == [[U(1), U(2)]]  # no further RPC


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

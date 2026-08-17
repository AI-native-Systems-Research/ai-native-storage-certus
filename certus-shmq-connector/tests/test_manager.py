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
        self.reserve_fail: set[int] = set()
        self.copy_fail: set[int] = set()
        self.events: list[tuple[int, int]] = []
        self.dropped_count = 0

    def check(self, keys):
        keys = list(keys)
        self.calls.append(("check", keys))
        return [self.exists.get(k, False) for k in keys]

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


def test_key_to_u64_from_bytes_big_endian():
    assert _key_to_u64((1).to_bytes(8, "big")) == 1
    assert _key_to_u64((0xDEADBEEF).to_bytes(8, "big")) == 0xDEADBEEF
    assert _key_to_u64(42) == 42  # ints pass through


# ── offset math (KvCacheIpc) ──


def test_block_offset_includes_base_delta_and_stride():
    kv = KvCacheIpc(handle_bytes=b"h" * 64, gpu_device_id=0, stride_bytes=2048, base_delta=512)
    assert kv.block_offset(0) == 512
    assert kv.block_offset(1) == 512 + 2048
    assert kv.block_offset(5) == 512 + 5 * 2048


# ── manager: lookup / touch ──


def test_lookup_maps_to_check():
    ring = FakeRing()
    ring.exists[7] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    assert mgr.lookup((7).to_bytes(8, "big")) is True
    assert mgr.lookup((8).to_bytes(8, "big")) is False


def test_touch_maps_to_touch_no_promote():
    ring = FakeRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    mgr.touch([(1).to_bytes(8, "big"), (2).to_bytes(8, "big")])
    (args,) = _calls_of(ring, "touch")
    keys, promote = args
    assert keys == [1, 2]
    assert promote is False


# ── manager: prepare_store ──


def test_prepare_store_filters_existing_and_reserves():
    ring = FakeRing()
    ring.exists[1] = True  # already cached -> filtered out
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=8192)
    keys = [(1).to_bytes(8, "big"), (2).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out is not None
    assert out.keys_to_store == [keys[1]]
    (entries,) = _calls_of(ring, "reserve")
    assert [e[0] for e in entries] == [2]
    assert [e[1] for e in entries] == [8192]  # size == block_size_bytes


def test_prepare_store_all_existing_is_noop():
    ring = FakeRing()
    ring.exists[1] = True
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    out = mgr.prepare_store([(1).to_bytes(8, "big")])
    assert out is not None
    assert out.keys_to_store == []
    assert _calls_of(ring, "reserve") == []


def test_prepare_store_partial_reserve_keeps_reserved_drops_failed():
    # Best-effort: reserve is per-key independent, so a partial failure stores
    # the keys that fit and drops the rest (rather than rejecting the whole
    # request, which triggers a vLLM retry+warning storm).
    ring = FakeRing()
    ring.reserve_fail = {3}  # key 3 fails to reserve; key 2 succeeds
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [(2).to_bytes(8, "big"), (3).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out is not None
    # Only the reserved key is offered for storage, in offload order.
    assert out.keys_to_store == [keys[0]]
    assert out.store_spec.keys == [2]
    # The reserved key is kept (to be committed later), so no rollback; the
    # failed key allocated nothing, so it needs no abort either.
    assert _calls_of(ring, "abort_store") == []


def test_prepare_store_all_reserve_fail_returns_empty_not_none():
    # When nothing fits, return an empty (non-None) result so vLLM advances past
    # these tokens quietly instead of retrying and warning every scheduler step.
    ring = FakeRing()
    ring.reserve_fail = {2, 3}
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [(2).to_bytes(8, "big"), (3).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out is not None
    assert out.keys_to_store == []
    assert out.store_spec.keys == []
    assert _calls_of(ring, "abort_store") == []


def test_prepare_store_preserves_offload_order_in_partial():
    # store_spec must stay in offload order for the scheduler's positional zip
    # of src GPU block ids with dst keys to line up on a partial subset.
    ring = FakeRing()
    ring.reserve_fail = {20}  # drop the middle key
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    keys = [(10).to_bytes(8, "big"), (20).to_bytes(8, "big"), (30).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out.keys_to_store == [keys[0], keys[2]]
    assert out.store_spec.keys == [10, 30]


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
    spec = mgr.prepare_load([(4).to_bytes(8, "big"), (5).to_bytes(8, "big")])
    assert isinstance(spec, CertusLoadStoreSpec)
    assert spec.keys == [4, 5]
    (args,) = _calls_of(ring, "pin")
    keys, promote = args
    assert keys == [4, 5]
    # promote must be False: Lookup promotes cold entries itself; a Pin-promote
    # would race the Lookup-promote on mt.insert (AlreadyExists -> load crash).
    assert promote is False
    mgr.complete_load([(4).to_bytes(8, "big"), (5).to_bytes(8, "big")])
    (unpin,) = _calls_of(ring, "unpin")
    assert unpin == [4, 5]


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

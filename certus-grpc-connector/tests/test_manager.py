# SPDX-License-Identifier: Apache-2.0
"""Unit tests for the gRPC connector logic (no server, no GPU, no vLLM).

conftest.py installs fake vllm modules so these import cleanly.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor

import pytest

from certus_grpc_connector import dispatcher_pb2 as pb
from certus_grpc_connector.gpu import KvCacheIpc
from certus_grpc_connector.manager import GrpcCertusOffloadingManager, _key_to_u64
from certus_grpc_connector.mediums import BlockLocation, CertusLoadStoreSpec


# ── Fake DispatcherStub ──


class FakeStub:
    """Records requests and returns canned responses."""

    def __init__(self):
        self.calls: list[tuple[str, object]] = []
        self.exists: dict[int, bool] = {}
        self.reserve_fail: set[int] = set()
        self.copy_fail: set[int] = set()
        self.events: list[pb.EvictionEvent] = []
        self.dropped_count = 0

    def _entry_results(self, keys, fail=()):
        return pb.BatchPopulateResponse()  # unused placeholder

    def Check(self, req):
        self.calls.append(("Check", req))
        return pb.BatchCheckResponse(
            results=[pb.CheckResult(key=k, exists=self.exists.get(k, False)) for k in req.keys]
        )

    def Reserve(self, req):
        self.calls.append(("Reserve", req))
        results = []
        for e in req.entries:
            ok = e.key not in self.reserve_fail
            results.append(pb.EntryResult(key=e.key, success=ok))
        return pb.BatchReserveResponse(results=results)

    def CommitStore(self, req):
        self.calls.append(("CommitStore", req))
        return pb.BatchCommitStoreResponse(
            results=[pb.EntryResult(key=k, success=True) for k in req.keys]
        )

    def AbortStore(self, req):
        self.calls.append(("AbortStore", req))
        return pb.BatchAbortStoreResponse(
            results=[pb.EntryResult(key=k, success=True) for k in req.keys]
        )

    def Pin(self, req):
        self.calls.append(("Pin", req))
        return pb.BatchPinResponse(
            results=[pb.EntryResult(key=k, success=True) for k in req.keys]
        )

    def Unpin(self, req):
        self.calls.append(("Unpin", req))
        return pb.BatchUnpinResponse(
            results=[pb.EntryResult(key=k, success=True) for k in req.keys]
        )

    def Touch(self, req):
        self.calls.append(("Touch", req))
        return pb.BatchTouchResponse(
            results=[pb.EntryResult(key=k, success=True) for k in req.keys]
        )

    def CopyToStore(self, req):
        self.calls.append(("CopyToStore", req))
        return pb.BatchCopyToStoreResponse(
            results=[
                pb.EntryResult(key=e.key, success=e.key not in self.copy_fail)
                for e in req.entries
            ]
        )

    def Lookup(self, req):
        self.calls.append(("Lookup", req))
        return pb.BatchLookupResponse(
            results=[pb.EntryResult(key=e.key, success=True) for e in req.entries]
        )

    def TakeEvents(self, req):
        self.calls.append(("TakeEvents", req))
        # The real server drains its queue on each call; mirror that so a
        # second call returns nothing.
        events, self.events = self.events, []
        dropped, self.dropped_count = self.dropped_count, 0
        return pb.TakeEventsResponse(events=events, dropped_count=dropped)


def _calls_of(stub, name):
    return [req for n, req in stub.calls if n == name]


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
    stub = FakeStub()
    stub.exists[7] = True
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    assert mgr.lookup((7).to_bytes(8, "big")) is True
    assert mgr.lookup((8).to_bytes(8, "big")) is False


def test_touch_maps_to_touch_rpc_no_promote():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr.touch([(1).to_bytes(8, "big"), (2).to_bytes(8, "big")])
    (req,) = _calls_of(stub, "Touch")
    assert list(req.keys) == [1, 2]
    assert req.promote is False


# ── manager: prepare_store ──


def test_prepare_store_filters_existing_and_reserves():
    stub = FakeStub()
    stub.exists[1] = True  # already cached -> filtered out
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=8192)
    keys = [(1).to_bytes(8, "big"), (2).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out is not None
    assert out.keys_to_store == [keys[1]]
    (reserve,) = _calls_of(stub, "Reserve")
    assert [e.key for e in reserve.entries] == [2]
    assert [e.size for e in reserve.entries] == [8192]


def test_prepare_store_all_existing_is_noop():
    stub = FakeStub()
    stub.exists[1] = True
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    out = mgr.prepare_store([(1).to_bytes(8, "big")])
    assert out is not None
    assert out.keys_to_store == []
    assert _calls_of(stub, "Reserve") == []


def test_prepare_store_returns_none_on_reserve_failure_and_rolls_back():
    stub = FakeStub()
    stub.reserve_fail = {3}  # key 3 fails to reserve
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    keys = [(2).to_bytes(8, "big"), (3).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out is None
    # key 2 reserved successfully -> must be rolled back via AbortStore
    (abort,) = _calls_of(stub, "AbortStore")
    assert list(abort.keys) == [2]


# ── manager: complete_store / load ──


def test_complete_store_success_commits():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr.complete_store([(9).to_bytes(8, "big")], success=True)
    assert _calls_of(stub, "CommitStore")
    assert not _calls_of(stub, "AbortStore")


def test_complete_store_failure_aborts():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr.complete_store([(9).to_bytes(8, "big")], success=False)
    assert _calls_of(stub, "AbortStore")
    assert not _calls_of(stub, "CommitStore")


def test_prepare_load_pins_with_promote_and_complete_load_unpins():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    spec = mgr.prepare_load([(4).to_bytes(8, "big"), (5).to_bytes(8, "big")])
    assert isinstance(spec, CertusLoadStoreSpec)
    assert spec.keys == [4, 5]
    (pin,) = _calls_of(stub, "Pin")
    assert list(pin.keys) == [4, 5]
    assert pin.promote is True
    mgr.complete_load([(4).to_bytes(8, "big"), (5).to_bytes(8, "big")])
    (unpin,) = _calls_of(stub, "Unpin")
    assert list(unpin.keys) == [4, 5]


# ── manager: take_events ──


def test_take_events_surfaces_removed_not_demoted():
    stub = FakeStub()
    stub.events = [
        pb.EvictionEvent(key=100, reason=pb.EVICTION_REASON_REMOVED),
        pb.EvictionEvent(key=200, reason=pb.EVICTION_REASON_DEMOTED),
    ]
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    events = list(mgr.take_events())
    assert len(events) == 1
    assert events[0].removed is True
    assert events[0].keys == [(100).to_bytes(8, "big")]
    # second call drains the buffer
    assert list(mgr.take_events()) == []


# ── handler offset wiring ──


def test_store_handler_sends_offsets_per_block():
    from certus_grpc_connector.handler import GpuToCertusHandler
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    stub = FakeStub()
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=1, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)
    h = GpuToCertusHandler(stub, kv, block_size_bytes=1024, executor=executor)

    src = GPULoadStoreSpec(block_ids=[3, 7])
    dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
    assert h.transfer_async(job_id=1, spec=(src, dst)) is True
    h.wait({1})
    results = h.get_finished()
    assert len(results) == 1 and results[0].success

    (req,) = _calls_of(stub, "CopyToStore")
    assert [e.key for e in req.entries] == [30, 70]
    assert [e.ipc_handle.offset for e in req.entries] == [3 * 1024, 7 * 1024]
    assert all(e.ipc_handle.cuda_ipc_handle == b"z" * 64 for e in req.entries)
    assert all(e.ipc_handle.gpu_device_id == 1 for e in req.entries)
    executor.shutdown()


def test_store_handler_never_reports_failure_and_aborts_failed_keys():
    """Regression: a failed CopyToStore must NOT surface success=False (vLLM's
    offloading worker asserts transfer_result.success and crashes the engine).
    The failed keys are rolled back via AbortStore; the job reports success."""
    from certus_grpc_connector.handler import GpuToCertusHandler
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    stub = FakeStub()
    stub.copy_fail = {70}  # one of two blocks fails to copy
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=0, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)
    h = GpuToCertusHandler(stub, kv, block_size_bytes=1024, executor=executor)

    src = GPULoadStoreSpec(block_ids=[3, 7])
    dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
    h.transfer_async(job_id=9, spec=(src, dst))
    h.wait({9})
    results = h.get_finished()
    assert len(results) == 1 and results[0].success is True  # never False
    (abort,) = _calls_of(stub, "AbortStore")
    assert list(abort.keys) == [70]  # only the failed key rolled back
    executor.shutdown()

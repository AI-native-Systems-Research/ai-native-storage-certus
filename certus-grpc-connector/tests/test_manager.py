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
        self.pin_fail: set[int] = set()
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

    def StoreBatch(self, req):
        self.calls.append(("StoreBatch", req))
        results = []
        for e in req.entries:
            ok = e.key not in self.copy_fail
            results.append(pb.EntryResult(key=e.key, success=ok, error_message="" if ok else "fail"))
        return pb.StoreBatchResponse(results=results)

    def LoadBatch(self, req):
        self.calls.append(("LoadBatch", req))
        results = [pb.EntryResult(key=e.key, success=True) for e in req.entries]
        return pb.LoadBatchResponse(results=results)

    def Pin(self, req):
        self.calls.append(("Pin", req))
        return pb.BatchPinResponse(
            results=[pb.EntryResult(key=k, success=k not in self.pin_fail) for k in req.keys]
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


def test_lookup_uses_presence_and_inflight():
    from certus_grpc_connector.compat import lookup_result, lookup_result_pending

    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    # Initially everything is MISS (no RPC, just local sets).
    assert mgr.lookup((7).to_bytes(8, "big")) == lookup_result(False)
    # After complete_store, key is in _presence -> HIT.
    mgr._presence.add(7)
    assert mgr.lookup((7).to_bytes(8, "big")) == lookup_result(True)
    # In-flight key -> HIT_PENDING.
    mgr._inflight.add(8)
    assert mgr.lookup((8).to_bytes(8, "big")) == lookup_result_pending()
    # No RPCs issued — lookup is purely local.
    assert _calls_of(stub, "Check") == []


def test_touch_maps_to_touch_rpc_no_promote():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr.touch([(1).to_bytes(8, "big"), (2).to_bytes(8, "big")])
    (req,) = _calls_of(stub, "Touch")
    assert list(req.keys) == [1, 2]
    assert req.promote is False


# ── manager: prepare_store ──


def test_prepare_store_filters_presence_and_inflight():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=8192)
    mgr._presence.add(1)  # already stored -> filtered out
    keys = [(1).to_bytes(8, "big"), (2).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out is not None
    assert out.keys_to_store == [keys[1]]
    assert out.store_spec.keys == [2]
    # Key 2 is now in-flight.
    assert 2 in mgr._inflight
    # No RPCs issued by prepare_store (StoreBatch is called by the worker).
    assert _calls_of(stub, "Reserve") == []
    assert _calls_of(stub, "StoreBatch") == []


def test_prepare_store_all_existing_is_noop():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr._presence.add(1)
    out = mgr.prepare_store([(1).to_bytes(8, "big")])
    assert out is not None
    assert out.keys_to_store == []
    assert mgr._inflight == set()


def test_prepare_store_deduplicates_inflight():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    keys = [(2).to_bytes(8, "big"), (3).to_bytes(8, "big")]
    # First call: both go to inflight.
    out1 = mgr.prepare_store(keys)
    assert out1.keys_to_store == keys
    assert mgr._inflight == {2, 3}
    # Second call with same keys: all filtered (already in-flight).
    out2 = mgr.prepare_store(keys)
    assert out2.keys_to_store == []


def test_prepare_store_preserves_offload_order():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr._presence.add(20)  # middle key already stored
    keys = [(10).to_bytes(8, "big"), (20).to_bytes(8, "big"), (30).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out.keys_to_store == [keys[0], keys[2]]
    assert out.store_spec.keys == [10, 30]


# ── manager: complete_store / load ──


def test_complete_store_success_moves_inflight_to_presence():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr._inflight.add(9)
    mgr.complete_store([(9).to_bytes(8, "big")], success=True)
    assert 9 in mgr._presence
    assert 9 not in mgr._inflight
    # No RPCs — StoreBatch already committed on the server.
    assert _calls_of(stub, "CommitStore") == []


def test_complete_store_failure_removes_from_inflight():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr._inflight.add(9)
    mgr.complete_store([(9).to_bytes(8, "big")], success=False)
    assert 9 not in mgr._inflight
    assert 9 not in mgr._presence


def test_prepare_load_pins_and_complete_load_unpins():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    spec = mgr.prepare_load([(4).to_bytes(8, "big"), (5).to_bytes(8, "big")])
    assert isinstance(spec, CertusLoadStoreSpec)
    assert spec.keys == [4, 5]
    (pin,) = _calls_of(stub, "Pin")
    assert list(pin.keys) == [4, 5]
    assert pin.promote is False
    mgr.complete_load([(4).to_bytes(8, "big"), (5).to_bytes(8, "big")])
    (unpin,) = _calls_of(stub, "Unpin")
    assert list(unpin.keys) == [4, 5]


def test_prepare_load_pin_failure_invalidates_presence():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr._presence.update({4, 5})
    # Simulate key 5 evicted between lookup and prepare_load.
    stub.pin_fail = {5}
    mgr.prepare_load([(4).to_bytes(8, "big"), (5).to_bytes(8, "big")])
    # Key 5 removed from presence so next lookup returns MISS.
    assert 4 in mgr._presence
    assert 5 not in mgr._presence


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


def test_take_events_eviction_clears_presence_and_inflight():
    stub = FakeStub()
    stub.events = [pb.EvictionEvent(key=42, reason=pb.EVICTION_REASON_REMOVED)]
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr._presence.add(42)
    mgr._inflight.add(42)
    list(mgr.take_events())
    assert 42 not in mgr._presence
    assert 42 not in mgr._inflight


def test_take_events_dropped_clears_all_caches():
    stub = FakeStub()
    stub.dropped_count = 5
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    mgr._presence.update({1, 2, 3})
    mgr._inflight.update({4, 5})
    list(mgr.take_events())
    assert mgr._presence == set()
    assert mgr._inflight == set()


def test_full_store_lifecycle_hit_pending_to_hit():
    from certus_grpc_connector.compat import lookup_result, lookup_result_pending

    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key = (99).to_bytes(8, "big")
    # Before store: MISS.
    assert mgr.lookup(key) == lookup_result(False)
    # prepare_store puts key in-flight.
    out = mgr.prepare_store([key])
    assert out.keys_to_store == [key]
    assert mgr.lookup(key) == lookup_result_pending()
    # complete_store(success) promotes to presence.
    mgr.complete_store([key], success=True)
    assert mgr.lookup(key) == lookup_result(True)
    assert 99 not in mgr._inflight
    assert 99 in mgr._presence


# ── handler offset wiring ──


def test_store_handler_sends_store_batch_with_offsets():
    from certus_grpc_connector.handler import worker_class
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    stub = FakeStub()
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=1, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)
    h = worker_class()(stub, [kv], block_size_bytes=1024,
                       store_executor=executor, load_executor=executor)

    src = GPULoadStoreSpec(block_ids=[3, 7], group_sizes=[2], block_indices=[0])
    dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
    assert h.transfer_async(job_id=1, spec=(src, dst)) is True
    h.wait({1})
    results = h.get_finished()
    assert len(results) == 1 and results[0].success

    (req,) = _calls_of(stub, "StoreBatch")
    assert [e.key for e in req.entries] == [30, 70]
    assert all(len(e.ipc_handles) == 1 for e in req.entries)
    assert [e.ipc_handles[0].offset for e in req.entries] == [3 * 1024, 7 * 1024]
    assert all(e.ipc_handles[0].cuda_ipc_handle == b"z" * 64 for e in req.entries)
    assert all(e.ipc_handles[0].gpu_device_id == 1 for e in req.entries)
    executor.shutdown()


def test_store_handler_reports_success_even_on_partial_failure():
    """A partial StoreBatch failure must NOT surface success=False (vLLM's
    offloading worker asserts transfer_result.success and crashes the engine).
    Partial failures are logged but the job reports success."""
    from certus_grpc_connector.handler import worker_class
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    stub = FakeStub()
    stub.copy_fail = {70}  # one of two blocks fails server-side
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=0, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)
    h = worker_class()(stub, [kv], block_size_bytes=1024,
                       store_executor=executor, load_executor=executor)

    src = GPULoadStoreSpec(block_ids=[3, 7], group_sizes=[2], block_indices=[0])
    dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
    h.transfer_async(job_id=9, spec=(src, dst))
    h.wait({9})
    results = h.get_finished()
    assert len(results) == 1 and results[0].success is True
    executor.shutdown()

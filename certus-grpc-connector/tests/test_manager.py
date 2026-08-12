# SPDX-License-Identifier: Apache-2.0
"""Unit tests for the gRPC connector logic (no server, no GPU, no vLLM).

conftest.py installs fake vllm modules so these import cleanly.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

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
        self.pin_fail: set[int] = set()
        self.copy_fail: set[int] = set()
        self.commit_fail: set[int] = set()
        self.abort_fail: set[int] = set()
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
            results=[
                pb.EntryResult(key=k, success=k not in self.commit_fail)
                for k in req.keys
            ]
        )

    def AbortStore(self, req):
        self.calls.append(("AbortStore", req))
        return pb.BatchAbortStoreResponse(
            results=[
                pb.EntryResult(key=k, success=k not in self.abort_fail)
                for k in req.keys
            ]
        )

    def Pin(self, req):
        self.calls.append(("Pin", req))
        return pb.BatchPinResponse(
            results=[
                pb.EntryResult(
                    key=k,
                    success=k not in self.pin_fail,
                    error_code=(
                        pb.ERROR_CODE_UNSPECIFIED
                        if k not in self.pin_fail
                        else pb.ERROR_CODE_KEY_NOT_FOUND
                    ),
                )
                for k in req.keys
            ]
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


def test_lookup_skips_repeated_check_after_known_miss():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key = (8).to_bytes(8, "big")

    assert mgr.lookup(key) is False
    assert mgr.lookup(key) is False

    checks = _calls_of(stub, "Check")
    assert len(checks) == 1
    assert list(checks[0].keys) == [8]


def test_lookup_many_batches_check_and_preserves_order():
    stub = FakeStub()
    stub.exists[1] = True
    stub.exists[3] = True
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    keys = [
        (1).to_bytes(8, "big"),
        (2).to_bytes(8, "big"),
        (3).to_bytes(8, "big"),
    ]

    assert mgr.lookup_many(keys) == [True, False, True]

    checks = _calls_of(stub, "Check")
    assert len(checks) == 1
    assert list(checks[0].keys) == [1, 2, 3]


def test_lookup_many_skips_known_miss_in_batch():
    stub = FakeStub()
    stub.exists[2] = True
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key1 = (1).to_bytes(8, "big")
    key2 = (2).to_bytes(8, "big")

    assert mgr.lookup(key1) is False
    stub.calls.clear()

    assert mgr.lookup_many([key1, key2]) == [False, True]

    checks = _calls_of(stub, "Check")
    assert len(checks) == 1
    assert list(checks[0].keys) == [2]


def test_lookup_rechecks_after_failed_commit_clears_hint():
    stub = FakeStub()
    stub.commit_fail = {46}
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key = (46).to_bytes(8, "big")

    assert mgr.lookup(key) is False
    out = mgr.prepare_store([key])
    assert out.keys_to_store == [key]
    mgr.complete_store([key], success=True)
    stub.calls.clear()

    assert mgr.lookup(key) is False

    assert _calls_of(stub, "Check")


def test_assume_lookup_hit_flag_skips_check(monkeypatch):
    monkeypatch.setenv("CERTUS_GRPC_ASSUME_LOOKUP_HIT", "1")
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)

    assert mgr.lookup((99).to_bytes(8, "big")) is True
    assert _calls_of(stub, "Check") == []


def test_pin_on_lookup_uses_pin_and_prepare_load_reuses_ref(monkeypatch):
    monkeypatch.setenv("CERTUS_GRPC_PIN_ON_LOOKUP", "1")
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    req = SimpleNamespace(req_id="req-1", kv_transfer_params=None)
    key = (7).to_bytes(8, "big")

    assert mgr.lookup(key, req) is True
    assert _calls_of(stub, "Check") == []
    assert len(_calls_of(stub, "Pin")) == 1
    assert list(_calls_of(stub, "Pin")[0].keys) == [7]

    spec = mgr.prepare_load([key], req)
    assert spec.keys == [7]
    assert len(_calls_of(stub, "Pin")) == 1

    mgr.complete_load([key], req)
    (unpin,) = _calls_of(stub, "Unpin")
    assert list(unpin.keys) == [7]


def test_pin_on_lookup_releases_unused_speculative_pin(monkeypatch):
    monkeypatch.setenv("CERTUS_GRPC_PIN_ON_LOOKUP", "1")
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    req = SimpleNamespace(req_id="req-1", kv_transfer_params=None)
    key = (8).to_bytes(8, "big")

    assert mgr.lookup(key, req) is True
    mgr.on_request_finished(req)

    (unpin,) = _calls_of(stub, "Unpin")
    assert list(unpin.keys) == [8]


def test_pin_on_lookup_without_req_id_falls_back_to_check(monkeypatch):
    monkeypatch.setenv("CERTUS_GRPC_PIN_ON_LOOKUP", "1")
    stub = FakeStub()
    stub.exists[9] = True
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)

    assert mgr.lookup((9).to_bytes(8, "big")) is True

    assert _calls_of(stub, "Pin") == []
    (check,) = _calls_of(stub, "Check")
    assert list(check.keys) == [9]


def test_pin_on_lookup_miss_records_negative_hint(monkeypatch):
    monkeypatch.setenv("CERTUS_GRPC_PIN_ON_LOOKUP", "1")
    stub = FakeStub()
    stub.pin_fail = {10}
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    req = SimpleNamespace(req_id="req-1", kv_transfer_params=None)
    key = (10).to_bytes(8, "big")

    assert mgr.lookup(key, req) is False
    assert mgr.lookup(key, req) is False

    assert len(_calls_of(stub, "Pin")) == 1
    assert _calls_of(stub, "Unpin") == []


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


def test_prepare_store_partial_reserve_keeps_reserved_drops_failed():
    # Best-effort: reserve is per-key independent, so a partial failure stores
    # the keys that fit and drops the rest (rather than rejecting the whole
    # request, which triggers a vLLM retry+warning storm).
    stub = FakeStub()
    stub.reserve_fail = {3}  # key 3 fails to reserve; key 2 succeeds
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    keys = [(2).to_bytes(8, "big"), (3).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out is not None
    # Only the reserved key is offered for storage, in offload order.
    assert out.keys_to_store == [keys[0]]
    assert out.store_spec.keys == [2]
    # The reserved key is kept (to be committed later), so no rollback; the
    # failed key allocated nothing, so it needs no abort either.
    assert _calls_of(stub, "AbortStore") == []


def test_prepare_store_all_reserve_fail_returns_empty_not_none():
    # When nothing fits, return an empty (non-None) result so vLLM advances past
    # these tokens quietly instead of retrying and warning every scheduler step.
    stub = FakeStub()
    stub.reserve_fail = {2, 3}
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    keys = [(2).to_bytes(8, "big"), (3).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out is not None
    assert out.keys_to_store == []
    assert out.store_spec.keys == []
    assert _calls_of(stub, "AbortStore") == []


def test_prepare_store_preserves_offload_order_in_partial():
    # store_spec must stay in offload order for the scheduler's positional zip
    # of src GPU block ids with dst keys to line up on a partial subset.
    stub = FakeStub()
    stub.reserve_fail = {20}  # drop the middle key
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    keys = [(10).to_bytes(8, "big"), (20).to_bytes(8, "big"), (30).to_bytes(8, "big")]
    out = mgr.prepare_store(keys)
    assert out.keys_to_store == [keys[0], keys[2]]
    assert out.store_spec.keys == [10, 30]


def test_prepare_store_skips_check_after_known_lookup_miss():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key = (44).to_bytes(8, "big")

    assert mgr.lookup(key) is False
    out = mgr.prepare_store([key])

    assert out is not None
    assert out.keys_to_store == [key]
    assert out.store_spec.keys == [44]
    # One Check from lookup(); prepare_store should trust the local miss hint and
    # go straight to Reserve.
    assert len(_calls_of(stub, "Check")) == 1
    assert _calls_of(stub, "Reserve")


def test_prepare_store_skips_check_for_known_present_key():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key = (45).to_bytes(8, "big")

    out = mgr.prepare_store([key])
    assert out.keys_to_store == [key]
    mgr.complete_store([key], success=True)
    stub.calls.clear()

    out = mgr.prepare_store([key])

    assert out is not None
    assert out.keys_to_store == []
    assert _calls_of(stub, "Check") == []
    assert _calls_of(stub, "Reserve") == []


def test_failed_commit_does_not_create_known_present_hint():
    stub = FakeStub()
    stub.commit_fail = {46}
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key = (46).to_bytes(8, "big")

    out = mgr.prepare_store([key])
    assert out.keys_to_store == [key]
    mgr.complete_store([key], success=True)
    stub.calls.clear()

    out = mgr.prepare_store([key])

    assert out is not None
    assert out.keys_to_store == [key]
    # A failed commit is not authoritative local state. The next store attempt
    # must ask the server again rather than skipping Check as "known present".
    assert _calls_of(stub, "Check")
    assert _calls_of(stub, "Reserve")


def test_prepare_store_skips_same_u64_collision():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key_a = (42).to_bytes(8, "big") + b"a" * 24
    key_b = (42).to_bytes(8, "big") + b"b" * 24

    out = mgr.prepare_store([key_a, key_b])

    assert out is not None
    assert out.keys_to_store == [key_a]
    assert out.store_spec.keys == [42]
    (reserve,) = _calls_of(stub, "Reserve")
    assert [e.key for e in reserve.entries] == [42]


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


def test_lookup_treats_same_u64_collision_as_miss():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    key_a = (42).to_bytes(8, "big") + b"a" * 24
    key_b = (42).to_bytes(8, "big") + b"b" * 24

    out = mgr.prepare_store([key_a])
    assert out.keys_to_store == [key_a]
    mgr.complete_store([key_a], success=True)
    stub.exists[42] = True

    assert mgr.lookup(key_a) is True
    assert mgr.lookup(key_b) is False


def test_prepare_load_pins_with_promote_and_complete_load_unpins():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    spec = mgr.prepare_load([(4).to_bytes(8, "big"), (5).to_bytes(8, "big")])
    assert isinstance(spec, CertusLoadStoreSpec)
    assert spec.keys == [4, 5]
    (pin,) = _calls_of(stub, "Pin")
    assert list(pin.keys) == [4, 5]
    # promote must be False: Lookup promotes cold entries itself; a Pin-promote
    # would race the Lookup-promote on mt.insert (AlreadyExists -> load crash).
    assert pin.promote is False
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


def test_take_events_surfaces_original_full_key_when_known():
    stub = FakeStub()
    mgr = GrpcCertusOffloadingManager(stub, block_size_bytes=4096)
    full_key = (100).to_bytes(8, "big") + b"full-vllm-offload-key"
    out = mgr.prepare_store([full_key])
    assert out.keys_to_store == [full_key]
    stub.events = [
        pb.EvictionEvent(key=100, reason=pb.EVICTION_REASON_REMOVED),
    ]

    events = list(mgr.take_events())

    assert len(events) == 1
    assert events[0].keys == [full_key]


# ── handler offset wiring ──


def test_store_handler_sends_offsets_per_block():
    from certus_grpc_connector.handler import worker_class
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    stub = FakeStub()
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=1, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)
    # One worker serves both directions; transfer_async routes a store by the
    # source spec being a GPULoadStoreSpec (≤0.24 medium-pair entrypoint). The
    # worker holds a LIST of KV regions (N==1 here — single-tensor block).
    h = worker_class()(stub, [kv], block_size_bytes=1024, executor=executor)

    src = GPULoadStoreSpec(block_ids=[3, 7])
    dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
    assert h.transfer_async(job_id=1, spec=(src, dst)) is True
    h.wait({1})
    results = h.get_finished()
    assert len(results) == 1 and results[0].success

    (req,) = _calls_of(stub, "CopyToStore")
    assert [e.key for e in req.entries] == [30, 70]
    # Single-region (N==1): the one region lands in ipc_handles[0].
    assert all(len(e.ipc_handles) == 1 for e in req.entries)
    assert [e.ipc_handles[0].offset for e in req.entries] == [3 * 1024, 7 * 1024]
    assert all(e.ipc_handles[0].cuda_ipc_handle == b"z" * 64 for e in req.entries)
    assert all(e.ipc_handles[0].gpu_device_id == 1 for e in req.entries)
    executor.shutdown()


def test_store_handler_never_reports_failure_and_aborts_failed_keys():
    """Regression: a failed CopyToStore must NOT surface success=False (vLLM's
    offloading worker asserts transfer_result.success and crashes the engine).
    The failed keys are rolled back via AbortStore; the job reports success."""
    from certus_grpc_connector.handler import worker_class
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    stub = FakeStub()
    stub.copy_fail = {70}  # one of two blocks fails to copy
    kv = KvCacheIpc(handle_bytes=b"z" * 64, gpu_device_id=0, stride_bytes=1024, base_delta=0)
    executor = ThreadPoolExecutor(max_workers=1)
    h = worker_class()(stub, [kv], block_size_bytes=1024, executor=executor)

    src = GPULoadStoreSpec(block_ids=[3, 7])
    dst = CertusLoadStoreSpec([BlockLocation(key=30), BlockLocation(key=70)])
    h.transfer_async(job_id=9, spec=(src, dst))
    h.wait({9})
    results = h.get_finished()
    assert len(results) == 1 and results[0].success is True  # never False
    (abort,) = _calls_of(stub, "AbortStore")
    assert list(abort.keys) == [70]  # only the failed key rolled back
    executor.shutdown()

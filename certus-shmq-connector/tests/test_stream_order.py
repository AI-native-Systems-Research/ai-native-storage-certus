"""Transfers wait for vLLM's compute stream before certus-server touches the blocks.

certus-server copies to and from vLLM's KV cache from another process, so it cannot be
ordered after vLLM's stream; under async scheduling a store could read KV the GPU had not
written yet (LongDocQA: the first documents' first token differed from recompute). The
worker records a mark on the submitting thread's stream; the worker's gate hands the job to
the transfer pool once the mark is ready, so no pool thread waits on the GPU.
These tests run without CUDA: a fake mark stands in for the CUDA event."""
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

import pytest

from certus_shmq_connector import handler


class _Mark:
    """Stands in for a torch.cuda.Event: synchronize() blocks until released."""

    def __init__(self, fail=False):
        self.released = threading.Event()
        self.waited = threading.Event()
        self.fail = fail

    def synchronize(self):
        self.waited.set()
        if self.fail:
            raise RuntimeError("CUDA error surfaced at synchronize")
        assert self.released.wait(5), "mark never released"

    def query(self):
        self.waited.set()
        if self.fail:
            raise RuntimeError("CUDA error surfaced at query")
        return self.released.is_set()


def _worker(transfer):
    w = handler.worker_class()(
        ring=SimpleNamespace(), kv_regions=[], block_size_bytes=4096,
        executor=ThreadPoolExecutor(max_workers=2))
    w._do_store = transfer
    w._do_load = transfer
    return w


def _spec(ids):
    return SimpleNamespace(block_ids=ids, group_sizes=[len(ids)], block_indices=[0], keys=[1] * len(ids))


def test_no_cuda_means_no_mark(monkeypatch):
    import builtins
    real_import = builtins.__import__

    def no_torch(name, *a, **kw):
        if name == "torch":
            raise ImportError("no torch here")
        return real_import(name, *a, **kw)

    monkeypatch.setattr(builtins, "__import__", no_torch)
    assert handler._compute_stream_mark() is None


@pytest.mark.parametrize("direction", ["store", "load"])
def test_transfer_starts_only_after_the_mark(monkeypatch, direction):
    mark = _Mark()
    monkeypatch.setattr(handler, "_compute_stream_mark", lambda: mark)
    started = threading.Event()

    def transfer(ids, keys, skip):
        started.set()
        return True

    w = _worker(transfer)
    if direction == "store":
        w.submit_store(1, _spec([0, 1]), _spec([0, 1]))
    else:
        w.submit_load(1, _spec([0, 1]), _spec([0, 1]))
    assert mark.waited.wait(5)
    time.sleep(0.05)
    assert not started.is_set(), "the server was asked to copy before the GPU reached the mark"
    mark.released.set()
    assert started.wait(5)
    deadline = time.monotonic() + 5
    done = []
    while not done and time.monotonic() < deadline:
        done = w.get_finished()
        time.sleep(0.01)
    assert [r.job_id for r in done] == [1] and done[0].success


def test_a_failed_wait_is_a_failed_job_not_a_crash(monkeypatch):
    monkeypatch.setattr(handler, "_compute_stream_mark", lambda: _Mark(fail=True))
    called = []
    w = _worker(lambda ids, keys, skip: called.append(1) or True)
    w.submit_store(7, _spec([0]), _spec([0]))
    deadline = time.monotonic() + 5
    done = []
    while not done and time.monotonic() < deadline:
        done = w.get_finished()
        time.sleep(0.01)
    assert [r.job_id for r in done] == [7]
    assert done[0].success is False and not called
    assert done[0].transfer_time == 0 or done[0].transfer_time >= 0


def test_transfer_async_routes_by_compats_current_spec_class():
    """transfer_async tells a store from a load with compat's GPULoadStoreSpec, read
    on each call: after compat is reloaded against another vLLM (the tests rebuild a
    fake per version), a class bound at handler's first import would no longer match
    and a store would be sent down the load path."""
    import importlib
    import conftest
    from certus_shmq_connector import compat
    from certus_shmq_connector.mediums import BlockLocation, CertusLoadStoreSpec

    w = handler.worker_class()(ring=SimpleNamespace(), kv_regions=[], block_size_bytes=4096,
                               executor=ThreadPoolExecutor(max_workers=1))
    seen = []
    w.submit_store = lambda job_id, src, dst: seen.append("store") or True
    w.submit_load = lambda job_id, src, dst: seen.append("load") or True
    try:
        conftest.build_fake_vllm((0, 20))      # a new GPULoadStoreSpec class
        importlib.reload(compat)
        from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
        gpu = GPULoadStoreSpec(block_ids=[0], group_sizes=[1], block_indices=[0])
        certus = CertusLoadStoreSpec([BlockLocation(key=1)])
        w.transfer_async(1, (gpu, certus))
        w.transfer_async(2, (certus, gpu))
        assert seen == ["store", "load"]
    finally:
        conftest.build_fake_vllm((0, 20))
        importlib.reload(compat)


def test_a_ready_load_is_not_held_behind_a_waiting_store(monkeypatch):
    """The gate hands each job to the pool as soon as its own mark is ready: a load
    whose blocks are written runs while a store still waits for a long prefill."""
    slow, fast = _Mark(), _Mark()
    fast.released.set()
    marks = iter([slow, fast])
    monkeypatch.setattr(handler, "_compute_stream_mark", lambda: next(marks))
    ran = []
    w = handler.worker_class()(ring=SimpleNamespace(), kv_regions=[], block_size_bytes=4096,
                               executor=ThreadPoolExecutor(max_workers=1))
    w._do_store = lambda ids, keys, skip: ran.append("store") or True
    w._do_load = lambda ids, keys, skip: ran.append("load") or True
    w.submit_store(1, _spec([0]), _spec([0]))     # waits on `slow`
    w.submit_load(2, _spec([1]), _spec([1]))      # `fast` is already ready
    deadline = time.monotonic() + 5
    while "load" not in ran and time.monotonic() < deadline:
        time.sleep(0.01)
    assert ran == ["load"], "the ready load waited behind the store"
    slow.released.set()
    while "store" not in ran and time.monotonic() < deadline:
        time.sleep(0.01)
    assert ran == ["load", "store"]


def _gate_with(executor):
    g = handler._ReadyGate(executor)
    return g


def test_gate_resolves_a_job_the_pool_rejects():
    ex = ThreadPoolExecutor(max_workers=1)
    ex.shutdown()                                # submit now raises RuntimeError
    g = _gate_with(ex)
    m = _Mark(); m.released.set()
    f = g.submit(m, lambda: True)
    with pytest.raises(RuntimeError):
        f.result(timeout=5)
    g.close(timeout=1)


def test_gate_resolves_a_cancelled_job():
    import concurrent.futures as cf
    ex = ThreadPoolExecutor(max_workers=1)
    block = threading.Event()
    ex.submit(block.wait)                        # occupy the only thread
    g = _gate_with(ex)
    m = _Mark(); m.released.set()
    f = g.submit(m, lambda: True)
    deadline = time.monotonic() + 5
    while g._held and time.monotonic() < deadline:   # the gate has forwarded it
        time.sleep(0.01)
    time.sleep(0.05)                             # and it is queued behind `block`
    ex.shutdown(wait=False, cancel_futures=True)
    block.set()
    with pytest.raises(cf.CancelledError):
        f.result(timeout=5)
    g.close(timeout=1)


def test_gate_close_dispatches_held_jobs_and_refuses_new_ones():
    g = _gate_with(ThreadPoolExecutor(max_workers=1))
    m = _Mark()
    f = g.submit(m, lambda: "ran")
    threading.Timer(0.1, m.released.set).start()  # becomes ready during close()
    g.close(timeout=5)
    assert f.result(timeout=5) == "ran"
    late = g.submit(_Mark(), lambda: "late")
    with pytest.raises(RuntimeError):
        late.result(timeout=1)
    assert not g._thread.is_alive()


def test_gate_close_fails_a_job_whose_mark_never_comes():
    g = _gate_with(ThreadPoolExecutor(max_workers=1))
    f = g.submit(_Mark(), lambda: "never")
    g.close(timeout=0.2)
    with pytest.raises(RuntimeError):
        f.result(timeout=5)


def _count_queries(n_jobs, seconds):
    queries = []

    class Counting(_Mark):
        def query(self):
            queries.append(1)
            return False

    g = _gate_with(ThreadPoolExecutor(max_workers=1))
    for _ in range(n_jobs):
        g.submit(Counting(), lambda: None)
    time.sleep(seconds)
    n = len(queries)
    g.close(timeout=0.05)
    return n / n_jobs                            # passes over the held jobs


def test_gate_polls_a_small_queue_at_the_fast_interval():
    # A few held jobs (multi-turn holds ~4): polled every ~50 us, no backoff, so a
    # ready job is dispatched promptly. 0.3 s at 50 us is ~6,000 passes; at 1 ms ~300.
    passes = _count_queries(4, 0.3)
    assert passes > 1000, passes


def test_gate_backs_off_with_many_held_jobs():
    # Above _BACKOFF_HELD the interval doubles to 1 ms, bounding the query work.
    passes = _count_queries(handler._ReadyGate._BACKOFF_HELD + 16, 0.5)
    assert passes < 700, passes

"""Transfers wait for vLLM's compute stream before certus-server touches the blocks.

certus-server copies to and from vLLM's KV cache from another process, so it cannot be
ordered after vLLM's stream; under async scheduling a store could read KV the GPU had not
written yet (LongDocQA: the first documents' first token differed from recompute). The
worker records a mark on the submitting thread's stream and the pool thread waits for it.
These tests run without CUDA: a fake mark stands in for the CUDA event."""
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

import pytest


def _handler():
    # Imported inside each test, not at collection: certus_shmq_connector.handler binds
    # vLLM classes (GPULoadStoreSpec) from the fake vLLM present at its first import,
    # and test_manager's tests compare against the fake they install themselves.
    from certus_shmq_connector import handler
    return handler


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


def _worker(transfer):
    # A class of its own, not handler.worker_class(): that caches the class against the
    # fake vLLM installed now, and later tests install another (isinstance checks in
    # transfer_async would then fail against the cached class).
    w = _handler()._build_worker_class()(
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
    h = _handler()
    assert h._compute_stream_mark() is None


@pytest.mark.parametrize("direction", ["store", "load"])
def test_transfer_starts_only_after_the_mark(monkeypatch, direction):
    mark = _Mark()
    monkeypatch.setattr(_handler(), "_compute_stream_mark", lambda: mark)
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
    monkeypatch.setattr(_handler(), "_compute_stream_mark", lambda: _Mark(fail=True))
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

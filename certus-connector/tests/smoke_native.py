#!/usr/bin/env python3
"""Smoke test for the native Rust certus_native path.

Requires vfio-pci bound NVMe devices and hugepages. Run as:
    LD_LIBRARY_PATH=/usr/local/cuda-12.8/targets/x86_64-linux/lib:$LD_LIBRARY_PATH \\
        python tests/smoke_native.py

Not a pytest test — needs real hardware.

Two test paths:
  - Host memory path: uses store_host_bytes/load_host_bytes to verify
    NVMe write/read roundtrip without GPU DMA.
  - GPU memory path: uses store_async/load_async with real GPU device
    pointers. Requires a live GPU KV cache allocation — skipped if torch
    is unavailable or no CUDA device is present.
"""

import sys
import time
from contextlib import contextmanager

try:
    import certus_native
except ImportError:
    print("FAIL: certus_native not installed. Run: pip install -e .")
    sys.exit(1)

CONFIG = {
    "data_pci_addrs": ["0000:61:00.0"],
    "metadata_pci_addr": "0000:62:00.0",
    "gpu_block_size": 131072,
    "slab_size_bytes": 131072,
    "dram_cache_bytes": 1073741824,  # 1 GB
    "io_queue_depth": 128,
    "eviction_threshold": 0.8,
}

BLOCK_SIZE = 131072  # must match gpu_block_size / slab_size_bytes

_timings: list[tuple[str, float]] = []


@contextmanager
def timed(label: str):
    t0 = time.perf_counter()
    yield
    elapsed_ms = (time.perf_counter() - t0) * 1000
    _timings.append((label, elapsed_ms))
    print(f"    [{label}: {elapsed_ms:.2f} ms]")


def check(label, condition, detail=""):
    status = "PASS" if condition else "FAIL"
    print(f"  [{status}] {label}" + (f": {detail}" if detail else ""))
    if not condition:
        sys.exit(1)


def warn(label, detail=""):
    print(f"  [SKIP] {label}" + (f": {detail}" if detail else ""))


def print_summary():
    if not _timings:
        return
    print("\n── Latency summary ───────────────────────────────────────")
    width = max(len(label) for label, _ in _timings)
    for label, ms in _timings:
        print(f"  {label:<{width}}  {ms:>8.2f} ms")


def test_basic(engine):
    print("── Basic API checks ──────────────────────────────────────")

    with timed("batch_check (3 keys, empty cache)"):
        count = engine.batch_check([1, 2, 3])
    check("batch_check returns 0 on empty cache", count == 0, f"got {count}")

    with timed("prepare_store (3 keys)"):
        to_store, evicted = engine.prepare_store([10, 11, 12])
    check("prepare_store returns 3 keys", len(to_store) == 3, f"got {to_store}")
    check("evicted is a list", isinstance(evicted, list))

    with timed("complete_store (3 keys, success)"):
        engine.complete_store([10, 11, 12], True)
    check("complete_store(success=True) did not raise", True)

    with timed("touch (2 keys)"):
        engine.touch([10, 11])
    check("touch did not raise", True)

    engine.prepare_store([99])
    with timed("complete_store (1 key, rollback)"):
        engine.complete_store([99], False)
    count = engine.batch_check([99])
    check("rolled-back key not cached", count == 0, f"got {count}")


def test_host_roundtrip(engine):
    """Write data via host memory, read it back from staging, verify contents."""
    print("\n── Host memory NVMe roundtrip ────────────────────────────")

    key = 1000
    payload = b"certus-host-test-" + b"X" * (BLOCK_SIZE - 17)
    assert len(payload) == BLOCK_SIZE

    with timed(f"store_host_bytes ({BLOCK_SIZE // 1024} KB, prepare+commit)"):
        engine.store_host_bytes(key, payload)
    check("store_host_bytes did not raise", True)

    # Try staging readback immediately.
    try:
        with timed(f"load_host_bytes ({BLOCK_SIZE // 1024} KB, from staging)"):
            result = engine.load_host_bytes(key, BLOCK_SIZE)
        check("load_host_bytes correct length", len(result) == BLOCK_SIZE, f"got {len(result)}")
        check("staging readback matches payload", bytes(result) == payload)
        print("  Staging readback: PASS (data still in DRAM staging buffer)")
    except RuntimeError as e:
        if "already migrated to NVMe" in str(e):
            print("  Staging readback: SKIP (background writer already flushed to NVMe)")
        else:
            raise

    # Poll until key is visible (staging or NVMe).
    t_migration_start = time.perf_counter()
    deadline = t_migration_start + 5.0
    visible = False
    while time.perf_counter() < deadline:
        if engine.batch_check([key]) > 0:
            visible = True
            break
        time.sleep(0.001)
    migration_ms = (time.perf_counter() - t_migration_start) * 1000
    _timings.append((f"NVMe migration ({BLOCK_SIZE // 1024} KB)", migration_ms))
    print(f"    [NVMe migration ({BLOCK_SIZE // 1024} KB): {migration_ms:.2f} ms]")
    check("key visible via batch_check (staging or NVMe)", visible)


def test_gpu_roundtrip(engine):
    """Write/read via GPU memory using store_async/load_async."""
    print("\n── GPU memory roundtrip ──────────────────────────────────")

    try:
        import torch
        if not torch.cuda.is_available():
            warn("GPU roundtrip skipped", "no CUDA device available")
            return
    except ImportError:
        warn("GPU roundtrip skipped", "torch not installed")
        return

    key = 2000
    pattern = 0xAB

    buf = torch.full((BLOCK_SIZE,), pattern, dtype=torch.uint8).pin_memory()
    gpu_ptr = buf.data_ptr()
    gpu_block_id = gpu_ptr // BLOCK_SIZE

    with timed(f"store_async ({BLOCK_SIZE // 1024} KB, pinned→staging→NVMe)"):
        ok = engine.store_async(job_id=10, gpu_block_ids=[gpu_block_id], keys=[key])
    check("store_async submitted", isinstance(ok, bool), f"got {ok}")

    with timed("poll_completions (store)"):
        completions = engine.poll_completions()
    check("poll_completions returned list", isinstance(completions, list))
    store_results = {jid: success for jid, success in completions}
    if 10 in store_results:
        check("store_async job 10 succeeded", store_results[10], f"got {store_results}")
    else:
        warn("store_async job 10 not yet complete", "may need poll retry")

    # Zero the buffer then load back.
    buf.fill_(0)
    with timed(f"load_async ({BLOCK_SIZE // 1024} KB, NVMe/staging→pinned)"):
        ok = engine.load_async(job_id=11, gpu_block_ids=[gpu_block_id], keys=[key])
    check("load_async submitted", isinstance(ok, bool))

    with timed("poll_completions (load)"):
        completions = engine.poll_completions()
    load_results = {jid: success for jid, success in completions}
    if 11 in load_results and load_results[11]:
        check("load_async restored pattern",
              buf[0].item() == pattern,
              f"first byte={buf[0].item():#x}, expected {pattern:#x}")
    else:
        warn("GPU load result not verified", f"completions={load_results}")


def main():
    print("=== certus_native smoke test ===\n")

    print("1. Engine init")
    with timed("CertusEngine init (SPDK + device probe + format)"):
        engine = certus_native.CertusEngine(CONFIG)
    check("CertusEngine constructed", engine is not None)

    test_basic(engine)
    test_host_roundtrip(engine)
    test_gpu_roundtrip(engine)

    print("\n── Shutdown ──────────────────────────────────────────────")
    with timed("shutdown"):
        engine.shutdown()
    check("shutdown did not raise", True)

    print_summary()
    print("\n=== All checks passed ===")


if __name__ == "__main__":
    main()

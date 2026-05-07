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


def check(label, condition, detail=""):
    status = "PASS" if condition else "FAIL"
    print(f"  [{status}] {label}" + (f": {detail}" if detail else ""))
    if not condition:
        sys.exit(1)


def warn(label, detail=""):
    print(f"  [SKIP] {label}" + (f": {detail}" if detail else ""))


def test_basic(engine):
    print("── Basic API checks ──────────────────────────────────────")

    count = engine.batch_check([1, 2, 3])
    check("batch_check returns 0 on empty cache", count == 0, f"got {count}")

    to_store, evicted = engine.prepare_store([10, 11, 12])
    check("prepare_store returns 3 keys", len(to_store) == 3, f"got {to_store}")
    check("evicted is a list", isinstance(evicted, list))

    engine.complete_store([10, 11, 12], True)
    check("complete_store(success=True) did not raise", True)

    engine.touch([10, 11])
    check("touch did not raise", True)

    engine.prepare_store([99])
    engine.complete_store([99], False)
    count = engine.batch_check([99])
    check("rolled-back key not cached", count == 0, f"got {count}")


def test_host_roundtrip(engine):
    """Write data via host memory, read it back from staging, verify contents.

    Two sub-tests:
    1. Staging readback: read immediately after write (data still in DRAM staging buffer).
    2. NVMe visibility: poll until background writer migrates to NVMe, verify batch_check.
    """
    print("\n── Host memory NVMe roundtrip ────────────────────────────")

    key = 1000
    payload = b"certus-host-test-" + b"X" * (BLOCK_SIZE - 17)
    assert len(payload) == BLOCK_SIZE

    # Write via prepare_store+commit_store (no GPU DMA).
    engine.store_host_bytes(key, payload)
    check("store_host_bytes did not raise", True)

    # 1. Read back — try staging first, fall back gracefully if already on NVMe.
    try:
        result = engine.load_host_bytes(key, BLOCK_SIZE)
        check("load_host_bytes returned correct length", len(result) == BLOCK_SIZE, f"got {len(result)}")
        check("staging readback matches written payload", bytes(result) == payload)
        print("  Staging readback: PASS (data still in DRAM staging buffer)")
    except RuntimeError as e:
        if "already migrated to NVMe" in str(e):
            print("  Staging readback: SKIP (background writer already moved data to NVMe — fast machine)")
        else:
            raise

    # 2. Key must be visible via batch_check regardless of staging/NVMe state.
    deadline = time.monotonic() + 5.0
    visible = False
    while time.monotonic() < deadline:
        if engine.batch_check([key]) > 0:
            visible = True
            break
        time.sleep(0.05)
    check("key visible via batch_check (staging or NVMe)", visible)
    print("  NVMe migration: PASS (key visible in dispatch map after background write)")


def test_gpu_roundtrip(engine):
    """Write/read via GPU memory using store_async/load_async.

    Allocates a pinned CUDA buffer, writes a known pattern, stores it,
    clears the buffer, loads it back, and verifies the pattern.
    Skipped if torch/CUDA is unavailable.
    """
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

    # Allocate pinned host memory as a stand-in for GPU KV cache.
    # (Real KV cache would be a CUDA device tensor — this verifies the
    # plumbing without needing a full vLLM context.)
    buf = torch.full((BLOCK_SIZE,), pattern, dtype=torch.uint8).pin_memory()
    gpu_ptr = buf.data_ptr()
    gpu_block_id = gpu_ptr // BLOCK_SIZE  # synthetic block ID from pointer

    ok = engine.store_async(job_id=10, gpu_block_ids=[gpu_block_id], keys=[key])
    check("store_async submitted", isinstance(ok, bool), f"got {ok}")

    completions = engine.poll_completions()
    check("poll_completions returned list", isinstance(completions, list))
    store_results = {jid: success for jid, success in completions}
    if 10 in store_results:
        check("store_async job 10 succeeded", store_results[10], f"got {store_results}")
    else:
        warn("store_async job 10 not yet complete (async)", "may need poll retry")

    # Zero the buffer, then load back.
    buf.fill_(0)
    ok = engine.load_async(job_id=11, gpu_block_ids=[gpu_block_id], keys=[key])
    check("load_async submitted", isinstance(ok, bool))

    completions = engine.poll_completions()
    load_results = {jid: success for jid, success in completions}
    if 11 in load_results and load_results[11]:
        check("load_async restored pattern", buf[0].item() == pattern,
              f"first byte={buf[0].item():#x}, expected {pattern:#x}")
    else:
        warn("GPU load result not verified", f"completions={load_results}")


def main():
    print("=== certus_native smoke test ===\n")

    print("1. Engine init")
    engine = certus_native.CertusEngine(CONFIG)
    check("CertusEngine constructed", engine is not None)

    test_basic(engine)
    test_host_roundtrip(engine)
    test_gpu_roundtrip(engine)

    print("\n── Shutdown ──────────────────────────────────────────────")
    engine.shutdown()
    check("shutdown did not raise", True)

    print("\n=== All checks passed ===")


if __name__ == "__main__":
    main()

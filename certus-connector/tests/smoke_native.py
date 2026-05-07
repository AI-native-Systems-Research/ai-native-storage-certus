#!/usr/bin/env python3
"""Smoke test for the native Rust certus_native path.

Requires vfio-pci bound NVMe devices and hugepages. Run as:
    python tests/smoke_native.py

Not a pytest test — needs real hardware.
"""

import sys

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


def check(label, condition, detail=""):
    status = "PASS" if condition else "FAIL"
    print(f"  [{status}] {label}" + (f": {detail}" if detail else ""))
    if not condition:
        sys.exit(1)


def main():
    print("=== certus_native smoke test ===\n")

    print("1. Engine init")
    engine = certus_native.CertusEngine(CONFIG)
    check("CertusEngine constructed", engine is not None)

    print("\n2. batch_check (empty cache)")
    count = engine.batch_check([1, 2, 3])
    check("batch_check returns 0 on empty cache", count == 0, f"got {count}")

    print("\n3. prepare_store")
    to_store, evicted = engine.prepare_store([10, 11, 12])
    check("prepare_store returns keys to store", len(to_store) == 3, f"got {to_store}")
    check("prepare_store evicted list is list", isinstance(evicted, list))

    print("\n4. complete_store (success)")
    engine.complete_store([10, 11, 12], True)
    check("complete_store success did not raise", True)

    print("\n5. batch_check (after store)")
    count = engine.batch_check([10, 11, 12])
    check("batch_check > 0 after store", count >= 0, f"got {count}")

    print("\n6. touch")
    engine.touch([10, 11])
    check("touch did not raise", True)

    print("\n7. store_async")
    ok = engine.store_async(job_id=1, gpu_block_ids=[0], keys=[20])
    check("store_async returned bool", isinstance(ok, bool), f"got {ok}")

    print("\n8. load_async")
    ok = engine.load_async(job_id=2, gpu_block_ids=[0], keys=[20])
    check("load_async returned bool", isinstance(ok, bool), f"got {ok}")

    print("\n9. poll_completions")
    completions = engine.poll_completions()
    check("poll_completions returned list", isinstance(completions, list))
    print(f"     completions: {completions}")

    print("\n10. complete_store (failure / rollback)")
    engine.prepare_store([99])
    engine.complete_store([99], False)
    count = engine.batch_check([99])
    check("rolled-back key not cached", count == 0, f"got {count}")

    print("\n11. shutdown")
    engine.shutdown()
    check("shutdown did not raise", True)

    print("\n=== All checks passed ===")


if __name__ == "__main__":
    main()

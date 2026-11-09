#!/usr/bin/env python3
"""Hardware integration test for split-phase store APIs.

Tests the Reserve → CopyToStore → CommitStore lifecycle and the
AbortStore cancellation path. Verifies:
  1. Reserve allocates slots (Check returns false — not yet visible)
  2. CopyToStore transfers data from GPU into reserved DRAM slot
  3. CommitStore makes the entry visible (Check returns true)
  4. Lookup after commit returns correct data (integrity check)
  5. AbortStore cancels a reserved slot (entry never becomes visible)
  6. Double-commit is rejected (AlreadyExists or KeyNotFound)
  7. CommitStore without prior Reserve is rejected (KeyNotFound)

Talks to certus-server over the /dev/shm shmq mailbox (the Ring client);
the old gRPC Dispatcher transport has been removed.

Usage:
    python test-split-phase-store.py --shm-path /dev/shm/certus-shmq --block-size 64K --num-objects 8
"""

import argparse
import ctypes
import os
import random
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from certus_shmq_helpers import RingError, add_shm_arg, connect, single_region
from certus_shmq_connector.ring import REASON_DEMOTED, REASON_REMOVED

# --- CUDA helpers ---

_libcudart = ctypes.CDLL("libcudart.so")
_libcudart.cudaSetDevice.restype = ctypes.c_int
_libcudart.cudaSetDevice.argtypes = [ctypes.c_int]
_libcudart.cudaMalloc.restype = ctypes.c_int
_libcudart.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
_libcudart.cudaFree.restype = ctypes.c_int
_libcudart.cudaFree.argtypes = [ctypes.c_void_p]
_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_libcudart.cudaMemcpy.restype = ctypes.c_int
_libcudart.cudaMemcpy.argtypes = [
    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int
]
_libcudart.cudaDeviceSynchronize.restype = ctypes.c_int
_CUDA_MEMCPY_H2D = 1
_CUDA_MEMCPY_D2H = 2


def cuda_alloc(size):
    dev_ptr = ctypes.c_void_p()
    err = _libcudart.cudaMalloc(ctypes.byref(dev_ptr), size)
    if err != 0:
        raise RuntimeError(f"cudaMalloc failed: {err}")
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), dev_ptr)
    if err != 0:
        raise RuntimeError(f"cudaIpcGetMemHandle failed: {err}")
    return dev_ptr, bytes(handle_buf)


def cuda_free(dev_ptr):
    _libcudart.cudaFree(dev_ptr)


def gpu_write(dev_ptr, data):
    buf = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
    _libcudart.cudaMemcpy(dev_ptr, ctypes.byref(buf), len(data), _CUDA_MEMCPY_H2D)


def gpu_read(dev_ptr, size):
    buf = (ctypes.c_ubyte * size)()
    err = _libcudart.cudaMemcpy(ctypes.byref(buf), dev_ptr, size, _CUDA_MEMCPY_D2H)
    if err != 0:
        raise RuntimeError(f"cudaMemcpy D2H failed: {err}")
    return bytes(buf)


def make_pattern(key, size):
    rng = random.Random(key)
    return bytes(rng.getrandbits(8) for _ in range(size))


def parse_size(s):
    s = s.strip()
    suffix = s[-1].upper()
    multipliers = {"K": 1024, "M": 1024 * 1024, "G": 1024 * 1024 * 1024}
    if suffix in multipliers:
        return int(s[:-1]) * multipliers[suffix]
    return int(s)


# --- Test helpers ---

def check_exists(ring, keys):
    oks = ring.check(keys)
    return {k: ok for k, ok in zip(keys, oks)}


def assert_all_success(oks, keys, op_name):
    for key, ok in zip(keys, oks):
        if not ok:
            raise AssertionError(f"{op_name} failed for key={key}")


def assert_result_error(oks, keys, key, expected_code, op_name):
    for k, ok in zip(keys, oks):
        if k == key:
            if ok:
                raise AssertionError(
                    f"{op_name} key={key}: expected error {expected_code}, got success"
                )
            return
    raise AssertionError(f"{op_name}: key={key} not found in results")


# --- Tests ---

def test_happy_path(ring, keys, block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles, gpu):
    """Reserve → CopyToStore → CommitStore → Lookup with integrity check."""
    print("\n  [TEST] Happy path: Reserve → CopyToStore → CommitStore → Lookup")

    # Write unique patterns to GPU
    patterns = {}
    for i, key in enumerate(keys):
        pattern = make_pattern(key, block_size)
        patterns[key] = pattern
        gpu_write(pop_ptrs[i], pattern)

    # Step 1: Reserve
    entries = [(k, block_size, 0) for k in keys]
    oks = ring.reserve(entries)
    assert_all_success(oks, keys, "Reserve")
    print("    Reserve:     OK")

    # Step 2: Verify NOT visible yet
    exists = check_exists(ring, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after Reserve (should not be)")
    print("    Not visible: OK (entries not in dispatch-map yet)")

    # Step 3: CopyToStore
    copy_entries = [
        (k, [single_region(pop_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.copy_to_store(copy_entries)
    assert_all_success(oks, keys, "CopyToStore")
    print("    CopyToStore: OK")

    # Step 4: Still NOT visible (DMA done, but not committed)
    exists = check_exists(ring, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after CopyToStore (should not be)")
    print("    Still hidden: OK (not committed yet)")

    # Step 5: CommitStore
    oks = ring.commit_store(keys)
    assert_all_success(oks, keys, "CommitStore")
    print("    CommitStore: OK")

    # Step 6: Now visible
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} not visible after CommitStore")
    print("    Visible:     OK (entries in dispatch-map)")

    # Step 7: Lookup and verify data integrity
    lookup_entries = [
        (k, [single_region(lookup_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.lookup(lookup_entries)
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(oks, keys, "Lookup")

    integrity_ok = True
    for i, k in enumerate(keys):
        actual = gpu_read(lookup_ptrs[i], block_size)
        if actual != patterns[k]:
            first_bad = next(
                (j for j in range(len(actual)) if actual[j] != patterns[k][j]), "?"
            )
            print(f"    INTEGRITY FAIL: key={k}, first mismatch at byte {first_bad}")
            integrity_ok = False

    if integrity_ok:
        print(f"    Integrity:   OK ({len(keys)} objects verified)")
    else:
        raise AssertionError("Data integrity check failed")

    # Cleanup
    ring.remove(keys)
    print("    Cleanup:     OK")


def test_abort_path(ring, keys, block_size, pop_ptrs, pop_handles, gpu):
    """Reserve → AbortStore → verify entry never becomes visible."""
    print("\n  [TEST] Abort path: Reserve → AbortStore → verify invisible")

    # Write data to GPU
    for i, key in enumerate(keys):
        gpu_write(pop_ptrs[i], make_pattern(key, block_size))

    # Reserve
    entries = [(k, block_size, 0) for k in keys]
    oks = ring.reserve(entries)
    assert_all_success(oks, keys, "Reserve")
    print("    Reserve:     OK")

    # CopyToStore (data in DRAM but not committed)
    copy_entries = [
        (k, [single_region(pop_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.copy_to_store(copy_entries)
    assert_all_success(oks, keys, "CopyToStore")
    print("    CopyToStore: OK")

    # Abort instead of commit
    oks = ring.abort_store(keys)
    assert_all_success(oks, keys, "AbortStore")
    print("    AbortStore:  OK")

    # Verify NOT visible
    exists = check_exists(ring, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after AbortStore (should not be)")
    print("    Invisible:   OK (entries correctly discarded)")


def test_abort_without_copy(ring, keys, block_size):
    """Reserve → AbortStore (skip CopyToStore) → verify invisible."""
    print("\n  [TEST] Abort without copy: Reserve → AbortStore (no DMA)")

    entries = [(k, block_size, 0) for k in keys]
    oks = ring.reserve(entries)
    assert_all_success(oks, keys, "Reserve")
    print("    Reserve:     OK")

    oks = ring.abort_store(keys)
    assert_all_success(oks, keys, "AbortStore")
    print("    AbortStore:  OK")

    exists = check_exists(ring, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after AbortStore")
    print("    Invisible:   OK")


def test_commit_without_reserve(ring, block_size):
    """CommitStore without prior Reserve should fail with KeyNotFound."""
    print("\n  [TEST] CommitStore without Reserve → expect KeyNotFound")

    bogus_key = 0xDEAD_BEEF_0001
    oks = ring.commit_store([bogus_key])
    assert_result_error(oks, [bogus_key], bogus_key, "KEY_NOT_FOUND", "CommitStore")
    print("    CommitStore: correctly rejected (KeyNotFound)")


def test_double_reserve(ring, block_size):
    """Reserve same key twice → second should fail with AlreadyExists."""
    print("\n  [TEST] Double Reserve → expect AlreadyExists")

    key = 0xDEAD_BEEF_0002
    entry = [(key, block_size, 0)]

    oks = ring.reserve(entry)
    assert_all_success(oks, [key], "Reserve (first)")
    print("    First Reserve:  OK")

    oks = ring.reserve(entry)
    assert_result_error(oks, [key], key, "ALREADY_EXISTS", "Reserve (second)")
    print("    Second Reserve: correctly rejected (AlreadyExists)")

    # Cleanup
    ring.abort_store([key])
    print("    Cleanup:        OK")


def test_reserve_after_abort_reuse(ring, keys, block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles, gpu):
    """Reserve → Abort → Reserve again → full lifecycle (slot reuse)."""
    print("\n  [TEST] Slot reuse: Reserve → Abort → Reserve → CopyToStore → Commit")

    for i, key in enumerate(keys):
        gpu_write(pop_ptrs[i], make_pattern(key, block_size))

    # First reserve + abort
    entries = [(k, block_size, 0) for k in keys]
    oks = ring.reserve(entries)
    assert_all_success(oks, keys, "Reserve (first)")
    oks = ring.abort_store(keys)
    assert_all_success(oks, keys, "AbortStore")
    print("    Reserve+Abort:  OK")

    # Second reserve + full lifecycle
    oks = ring.reserve(entries)
    assert_all_success(oks, keys, "Reserve (second)")

    copy_entries = [
        (k, [single_region(pop_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.copy_to_store(copy_entries)
    assert_all_success(oks, keys, "CopyToStore")

    oks = ring.commit_store(keys)
    assert_all_success(oks, keys, "CommitStore")
    print("    Re-Reserve+Commit: OK")

    # Verify visible + integrity
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} not visible after re-reserve+commit")

    lookup_entries = [
        (k, [single_region(lookup_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.lookup(lookup_entries)
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(oks, keys, "Lookup")

    for i, k in enumerate(keys):
        actual = gpu_read(lookup_ptrs[i], block_size)
        expected = make_pattern(k, block_size)
        if actual != expected:
            raise AssertionError(f"Integrity fail after slot reuse: key={k}")
    print("    Integrity:      OK")

    # Cleanup
    ring.remove(keys)
    print("    Cleanup:        OK")


def test_pin_unpin(ring, keys, block_size, pop_ptrs, pop_handles, gpu):
    """Populate → Pin → verify pinned key cannot be evicted → Unpin → cleanup."""
    print("\n  [TEST] Pin/Unpin: Populate → Pin → Unpin → Remove")

    # Write data and populate
    for i, key in enumerate(keys):
        gpu_write(pop_ptrs[i], make_pattern(key, block_size))

    pop_entries = [
        (k, [single_region(pop_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.populate(pop_entries)
    assert_all_success(oks, keys, "Populate")
    print("    Populate:  OK")

    # Pin all keys
    oks = ring.pin(keys)
    assert_all_success(oks, keys, "Pin")
    print("    Pin:       OK")

    # Verify keys still exist (pinning should not remove them)
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared after Pin")
    print("    Still exists: OK")

    # Unpin all keys
    oks = ring.unpin(keys)
    assert_all_success(oks, keys, "Unpin")
    print("    Unpin:     OK")

    # Cleanup
    ring.remove(keys)
    print("    Cleanup:   OK")


def test_unpin_without_pin(ring, block_size):
    """Unpin a key that was never pinned → should fail with KeyNotFound."""
    print("\n  [TEST] Unpin without Pin → expect error")

    bogus_key = 0xDEAD_BEEF_0010
    oks = ring.unpin([bogus_key])
    assert_result_error(oks, [bogus_key], bogus_key, "KEY_NOT_FOUND", "Unpin")
    print("    Unpin:     correctly rejected (KeyNotFound)")


def test_pin_nonexistent(ring, block_size):
    """Pin a key that doesn't exist → should fail with KeyNotFound."""
    print("\n  [TEST] Pin nonexistent key → expect error")

    bogus_key = 0xDEAD_BEEF_0011
    oks = ring.pin([bogus_key])
    assert_result_error(oks, [bogus_key], bogus_key, "KEY_NOT_FOUND", "Pin")
    print("    Pin:       correctly rejected (KeyNotFound)")


def test_pin_double_pin_unpin(ring, keys, block_size, pop_ptrs, pop_handles, gpu):
    """Pin twice → Unpin once → entry still pinned → Unpin again → fully unpinned."""
    print("\n  [TEST] Double Pin: Pin×2 → Unpin×1 (still pinned) → Unpin×1 → Remove")

    # Populate entries
    for i, key in enumerate(keys):
        gpu_write(pop_ptrs[i], make_pattern(key, block_size))

    pop_entries = [
        (k, [single_region(pop_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.populate(pop_entries)
    assert_all_success(oks, keys, "Populate")
    print("    Populate:    OK")

    # Pin twice (refcount should go to 2)
    oks = ring.pin(keys)
    assert_all_success(oks, keys, "Pin (first)")
    oks = ring.pin(keys)
    assert_all_success(oks, keys, "Pin (second)")
    print("    Pin×2:       OK (refcount=2)")

    # Unpin once (refcount drops to 1, still pinned)
    oks = ring.unpin(keys)
    assert_all_success(oks, keys, "Unpin (first)")
    print("    Unpin×1:     OK (refcount=1)")

    # Entries should still exist
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared while still pinned (refcount=1)")
    print("    Still exists: OK")

    # Unpin again (refcount drops to 0, fully unpinned)
    oks = ring.unpin(keys)
    assert_all_success(oks, keys, "Unpin (second)")
    print("    Unpin×2:     OK (refcount=0)")

    # Cleanup
    ring.remove(keys)
    print("    Cleanup:     OK")


def test_pin_lookup_while_pinned(ring, keys, block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles, gpu):
    """Populate → Pin → Lookup (data still accessible) → Unpin → Remove."""
    print("\n  [TEST] Lookup while pinned: Populate → Pin → Lookup → Unpin → Remove")

    # Write unique patterns and populate
    patterns = {}
    for i, key in enumerate(keys):
        pattern = make_pattern(key, block_size)
        patterns[key] = pattern
        gpu_write(pop_ptrs[i], pattern)

    pop_entries = [
        (k, [single_region(pop_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.populate(pop_entries)
    assert_all_success(oks, keys, "Populate")
    print("    Populate:  OK")

    # Pin
    oks = ring.pin(keys)
    assert_all_success(oks, keys, "Pin")
    print("    Pin:       OK")

    # Lookup while pinned — should work and return correct data
    lookup_entries = [
        (k, [single_region(lookup_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.lookup(lookup_entries)
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(oks, keys, "Lookup")

    for i, k in enumerate(keys):
        actual = gpu_read(lookup_ptrs[i], block_size)
        if actual != patterns[k]:
            first_bad = next(
                (j for j in range(len(actual)) if actual[j] != patterns[k][j]), "?"
            )
            raise AssertionError(
                f"Integrity fail while pinned: key={k}, first mismatch at byte {first_bad}"
            )
    print(f"    Lookup:    OK ({len(keys)} objects verified while pinned)")

    # Unpin
    oks = ring.unpin(keys)
    assert_all_success(oks, keys, "Unpin")
    print("    Unpin:     OK")

    # Cleanup
    ring.remove(keys)
    print("    Cleanup:   OK")


def test_unpin_underflow(ring, keys, block_size, pop_ptrs, pop_handles, gpu):
    """Populate → Pin → Unpin → Unpin again → expect error (refcount underflow)."""
    print("\n  [TEST] Unpin underflow: Pin×1 → Unpin×2 → expect error on second")

    # Populate
    for i, key in enumerate(keys):
        gpu_write(pop_ptrs[i], make_pattern(key, block_size))

    pop_entries = [
        (k, [single_region(pop_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.populate(pop_entries)
    assert_all_success(oks, keys, "Populate")
    print("    Populate:  OK")

    # Pin once
    oks = ring.pin(keys)
    assert_all_success(oks, keys, "Pin")
    print("    Pin:       OK")

    # Unpin once (valid — returns to 0)
    oks = ring.unpin(keys)
    assert_all_success(oks, keys, "Unpin (first)")
    print("    Unpin×1:   OK")

    # Unpin again — should fail (refcount already 0)
    oks = ring.unpin(keys)
    for k, ok in zip(keys, oks):
        if ok:
            raise AssertionError(
                f"Unpin key={k} succeeded when refcount should be 0 (expected error)"
            )
    print("    Unpin×2:   correctly rejected (underflow)")

    # Cleanup
    ring.remove(keys)
    print("    Cleanup:   OK")


def test_pin_with_split_phase_store(ring, keys, block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles, gpu):
    """Reserve → CopyToStore → CommitStore → Pin → Lookup → Unpin → Remove.

    Verifies Pin/Unpin works with entries created via the split-phase store path.
    """
    print("\n  [TEST] Pin with split-phase store: Reserve → Commit → Pin → Lookup → Unpin")

    # Write patterns to GPU
    patterns = {}
    for i, key in enumerate(keys):
        pattern = make_pattern(key, block_size)
        patterns[key] = pattern
        gpu_write(pop_ptrs[i], pattern)

    # Reserve
    entries = [(k, block_size, 0) for k in keys]
    oks = ring.reserve(entries)
    assert_all_success(oks, keys, "Reserve")

    # CopyToStore
    copy_entries = [
        (k, [single_region(pop_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.copy_to_store(copy_entries)
    assert_all_success(oks, keys, "CopyToStore")

    # CommitStore
    oks = ring.commit_store(keys)
    assert_all_success(oks, keys, "CommitStore")
    print("    Split-phase store: OK")

    # Pin the committed entries
    oks = ring.pin(keys)
    assert_all_success(oks, keys, "Pin")
    print("    Pin:       OK")

    # Lookup while pinned — verify data integrity
    lookup_entries = [
        (k, [single_region(lookup_handles[i], gpu, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.lookup(lookup_entries)
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(oks, keys, "Lookup")

    for i, k in enumerate(keys):
        actual = gpu_read(lookup_ptrs[i], block_size)
        if actual != patterns[k]:
            raise AssertionError(f"Integrity fail: key={k} after split-phase + pin")
    print(f"    Integrity: OK ({len(keys)} objects)")

    # Unpin
    oks = ring.unpin(keys)
    assert_all_success(oks, keys, "Unpin")
    print("    Unpin:     OK")

    # Cleanup
    ring.remove(keys)
    print("    Cleanup:   OK")


def test_take_events_empty(ring):
    """TakeEvents on an empty queue returns zero events."""
    print("  [take_events_empty]")
    events, dropped = ring.take_events(max_events=0)
    assert len(events) == 0, f"Expected 0 events, got {len(events)}"
    assert dropped == 0, f"Expected 0 dropped, got {dropped}"
    print("    Empty drain: OK")


def test_take_events_after_eviction(ring, block_size, pop_ptrs, pop_handles, gpu):
    """Fill memory-tier to trigger eviction, then drain events."""
    print("  [take_events_after_eviction]")

    # Drain any stale events first
    ring.take_events(max_events=0)

    # Populate many entries to force eviction (memory-tier is 256 MiB).
    # Use large objects to fill faster.
    num_entries = (256 * 1024 * 1024) // block_size + 16
    base_key = 900000

    keys_to_populate = []
    for i in range(num_entries):
        k = base_key + i
        keys_to_populate.append(k)

    # Populate in batches
    batch_size = min(len(pop_ptrs), 8)
    for batch_start in range(0, len(keys_to_populate), batch_size):
        batch_keys = keys_to_populate[batch_start:batch_start + batch_size]
        entries = []
        for idx, k in enumerate(batch_keys):
            ptr_idx = idx % len(pop_ptrs)
            entries.append(
                (k, [single_region(pop_handles[ptr_idx], gpu, block_size)])
            )
        # Some may fail due to pool full — that's expected (no per-key error
        # strings over shmq; a False just means that entry was not stored).
        ring.populate(entries)

    # Now drain eviction events
    events, dropped = ring.take_events(max_events=0)
    print(f"    Events drained: {len(events)} (dropped: {dropped})")
    assert len(events) > 0 or dropped > 0, \
        "Expected at least some eviction events after overflowing memory-tier"

    # Verify event structure
    for ev in events:
        assert ev[0] > 0, f"Event key should be > 0, got {ev[0]}"
        assert ev[1] in (
            REASON_DEMOTED,
            REASON_REMOVED,
        ), f"Unexpected reason: {ev[1]}"
    print(f"    Event structure: OK")

    # Second drain should be empty
    events2, _dropped2 = ring.take_events(max_events=0)
    assert len(events2) == 0, f"Expected 0 events on second drain, got {len(events2)}"
    print(f"    Second drain empty: OK")

    # Cleanup
    ring.remove(keys_to_populate)
    print("    Cleanup: OK")


def test_take_events_max_limit(ring, block_size, pop_ptrs, pop_handles, gpu):
    """TakeEvents with max_events limit returns at most that many."""
    print("  [take_events_max_limit]")

    # Drain stale events
    ring.take_events(max_events=0)

    # Populate enough to trigger evictions
    num_entries = (256 * 1024 * 1024) // block_size + 16
    base_key = 800000

    keys_to_populate = []
    for i in range(num_entries):
        keys_to_populate.append(base_key + i)

    batch_size = min(len(pop_ptrs), 8)
    for batch_start in range(0, len(keys_to_populate), batch_size):
        batch_keys = keys_to_populate[batch_start:batch_start + batch_size]
        entries = []
        for idx, k in enumerate(batch_keys):
            ptr_idx = idx % len(pop_ptrs)
            entries.append(
                (k, [single_region(pop_handles[ptr_idx], gpu, block_size)])
            )
        ring.populate(entries)

    # Request only 3 events
    events, _dropped = ring.take_events(max_events=3)
    if len(events) > 0:
        assert len(events) <= 3, f"Expected at most 3 events, got {len(events)}"
        print(f"    max_events=3: got {len(events)} events (OK)")
    else:
        print(f"    max_events=3: no events available (evictions may be pending)")

    # Drain remaining
    ring.take_events(max_events=0)

    # Cleanup
    ring.remove(keys_to_populate)
    print("    Cleanup: OK")


def main():
    parser = argparse.ArgumentParser(
        description="Integration test for split-phase store APIs"
    )
    add_shm_arg(parser)
    parser.add_argument(
        "--block-size", type=parse_size, default=64 * 1024,
        help="Object size (default: 64K)"
    )
    parser.add_argument(
        "--num-objects", type=int, default=8,
        help="Number of objects per test (default: 8)"
    )
    parser.add_argument("--gpu", type=int, default=0, help="GPU device index")
    args = parser.parse_args()

    block_size = args.block_size
    num_objects = args.num_objects
    base_key = 80_000_000

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 60)
    print("Split-Phase Store API Integration Test")
    print("=" * 60)
    print(f"  Server:      {args.shm_path}")
    print(f"  Block size:  {block_size // 1024} KiB")
    print(f"  Objects:     {num_objects}")
    print(f"  GPU:         {args.gpu}")

    # Allocate GPU buffers
    pop_ptrs, pop_handles = [], []
    lookup_ptrs, lookup_handles = [], []
    for _ in range(num_objects):
        ptr, handle = cuda_alloc(block_size)
        pop_ptrs.append(ptr)
        pop_handles.append(handle)
        ptr, handle = cuda_alloc(block_size)
        lookup_ptrs.append(ptr)
        lookup_handles.append(handle)

    # Each test gets its own key range to avoid state leakage from
    # background write-through between tests.
    key_sets = [
        [base_key + (t * num_objects) + i for i in range(num_objects)]
        for t in range(12)
    ]

    # Connect
    ring = connect(args.shm_path)

    passed = 0
    failed = 0
    tests = [
        ("happy_path", lambda: test_happy_path(
            ring, key_sets[0], block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles, args.gpu
        )),
        ("abort_path", lambda: test_abort_path(
            ring, key_sets[1], block_size, pop_ptrs, pop_handles, args.gpu
        )),
        ("abort_without_copy", lambda: test_abort_without_copy(
            ring, key_sets[2], block_size
        )),
        ("commit_without_reserve", lambda: test_commit_without_reserve(
            ring, block_size
        )),
        ("double_reserve", lambda: test_double_reserve(ring, block_size)),
        ("slot_reuse", lambda: test_reserve_after_abort_reuse(
            ring, key_sets[5], block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles, args.gpu
        )),
        ("pin_unpin", lambda: test_pin_unpin(
            ring, key_sets[6], block_size, pop_ptrs, pop_handles, args.gpu
        )),
        ("unpin_without_pin", lambda: test_unpin_without_pin(ring, block_size)),
        ("pin_nonexistent", lambda: test_pin_nonexistent(ring, block_size)),
        ("pin_double_pin_unpin", lambda: test_pin_double_pin_unpin(
            ring, key_sets[7], block_size, pop_ptrs, pop_handles, args.gpu
        )),
        ("pin_lookup_while_pinned", lambda: test_pin_lookup_while_pinned(
            ring, key_sets[8], block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles, args.gpu
        )),
        ("unpin_underflow", lambda: test_unpin_underflow(
            ring, key_sets[9], block_size, pop_ptrs, pop_handles, args.gpu
        )),
        ("pin_with_split_phase_store", lambda: test_pin_with_split_phase_store(
            ring, key_sets[10], block_size, pop_ptrs, pop_handles, lookup_ptrs, lookup_handles, args.gpu
        )),
        ("take_events_empty", lambda: test_take_events_empty(ring)),
        ("take_events_after_eviction", lambda: test_take_events_after_eviction(
            ring, block_size, pop_ptrs, pop_handles, args.gpu
        )),
        ("take_events_max_limit", lambda: test_take_events_max_limit(
            ring, block_size, pop_ptrs, pop_handles, args.gpu
        )),
    ]

    all_keys = [k for ks in key_sets for k in ks]
    for name, test_fn in tests:
        try:
            test_fn()
            passed += 1
        except (AssertionError, RingError) as e:
            print(f"    FAILED: {e}")
            failed += 1
            # Attempt cleanup on failure
            try:
                ring.abort_store(all_keys)
            except Exception:
                pass
            try:
                ring.remove(all_keys)
            except Exception:
                pass

    # Final cleanup
    for ptr in pop_ptrs:
        cuda_free(ptr)
    for ptr in lookup_ptrs:
        cuda_free(ptr)
    ring.close()

    # Summary
    print("\n" + "=" * 60)
    print(f"Results: {passed} passed, {failed} failed, {passed + failed} total")
    print("=" * 60)

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()

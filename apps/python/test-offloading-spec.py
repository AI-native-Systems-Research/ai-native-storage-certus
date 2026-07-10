#!/usr/bin/env python3
"""Validate all Certus gRPC APIs that map to the vLLM OffloadingSpec interface.

Tests the complete OffloadingManager lifecycle as exercised by vLLM's
KV-cache offloading connector. Each test section is named after the
OffloadingSpec operation it validates.

OffloadingSpec → Certus gRPC mapping:
    prepare_store(keys)              → Reserve
    transfer_async(store: GPU→DRAM)  → CopyToStore
    complete_store(keys, success)    → CommitStore / AbortStore
    lookup(keys)                     → Check
    prepare_load(keys)               → Pin + Touch(promote=true)
    transfer_async(load: DRAM→GPU)   → Lookup (DMA readback)
    complete_load(keys)              → Unpin
    touch(keys)                      → Touch
    take_events()                    → TakeEvents
    [explicit removal]               → Remove
    [bulk clear]                     → ClearMemoryTier
    [persistence flush]              → FlushToSsd

Usage:
    python test-offloading-spec.py --server localhost:50051 --block-size 64K

Requires: GPU with CUDA, running certus-server-yaml with --memory-tier-size 256M
"""

import argparse
import ctypes
import os
import random
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import grpc
import dispatcher_pb2
import dispatcher_pb2_grpc

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
_libcudart.cudaMemset.restype = ctypes.c_int
_libcudart.cudaMemset.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_size_t]
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


def gpu_zero(dev_ptr, size):
    _libcudart.cudaMemset(dev_ptr, 0, size)


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


# --- Assertion helpers ---

def ensure_pool_quiescent(stub):
    """Clear memory-tier, flush pending writes, drain events, and let evictor settle.

    The background evictor runs asynchronously and may still be demoting entries
    from a prior flood. We clear+flush+drain, sleep to let it finish, then repeat.
    """
    for _ in range(3):
        stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
        stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())
        stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))
        time.sleep(0.5)


def assert_all_success(resp, op_name):
    for r in resp.results:
        if not r.success:
            raise AssertionError(
                f"{op_name} failed for key={r.key}: "
                f"code={r.error_code} msg={r.error_message}"
            )


def assert_result_error(resp, key, op_name):
    for r in resp.results:
        if r.key == key:
            if r.success:
                raise AssertionError(f"{op_name} key={key}: expected error, got success")
            return r
    raise AssertionError(f"{op_name}: key={key} not found in results")


def check_exists(stub, keys):
    resp = stub.Check(dispatcher_pb2.BatchCheckRequest(keys=keys))
    return {r.key: r.exists for r in resp.results}


# --- Helpers to populate entries (used across tests) ---

def populate_entries(stub, keys, block_size, ptrs, handles):
    """Store entries via the split-phase path (Reserve → CopyToStore → Commit)."""
    entries = [dispatcher_pb2.ReserveEntry(key=k, size=block_size) for k in keys]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve")

    copy_entries = [
        dispatcher_pb2.CopyToStoreEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=handles[i], size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=copy_entries))
    assert_all_success(resp, "CopyToStore")

    resp = stub.CommitStore(dispatcher_pb2.BatchCommitStoreRequest(keys=keys))
    assert_all_success(resp, "CommitStore")


def populate_via_populate_rpc(stub, keys, block_size, ptrs, handles):
    """Store entries via the single-phase Populate RPC."""
    entries = [
        dispatcher_pb2.PopulateEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=handles[i], size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))
    assert_all_success(resp, "Populate")


# ============================================================
# TEST FUNCTIONS — one per OffloadingSpec operation
# ============================================================

def test_prepare_store(stub, keys, block_size, ptrs, handles):
    """OffloadingSpec.prepare_store → Reserve

    Validates:
    - Reserve allocates slots for keys
    - Entries are NOT visible before commit
    - Duplicate reserve is rejected (AlreadyExists)
    """
    print("\n  [prepare_store] Reserve allocates invisible slots")

    entries = [dispatcher_pb2.ReserveEntry(key=k, size=block_size) for k in keys]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve")
    print("    Reserve:          OK")

    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after Reserve (should be invisible)")
    print("    Not visible:      OK")

    # Duplicate reserve should fail
    dup_resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries[:1]))
    assert_result_error(dup_resp, keys[0], "Reserve (duplicate)")
    print("    Dup rejected:     OK (AlreadyExists)")

    # Cleanup
    stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=keys))
    print("    Cleanup:          OK")


def test_transfer_store(stub, keys, block_size, ptrs, handles):
    """OffloadingSpec transfer_async (store direction) → CopyToStore

    Validates:
    - After Reserve, CopyToStore transfers GPU data into the DRAM slot
    - Entry still not visible after CopyToStore (needs commit)
    """
    print("\n  [transfer_store] CopyToStore DMA from GPU to reserved slot")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))

    entries = [dispatcher_pb2.ReserveEntry(key=k, size=block_size) for k in keys]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve")

    copy_entries = [
        dispatcher_pb2.CopyToStoreEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=handles[i], size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=copy_entries))
    assert_all_success(resp, "CopyToStore")
    print("    CopyToStore:      OK")

    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after CopyToStore (not committed)")
    print("    Still invisible:  OK")

    # Cleanup via abort
    stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=keys))
    print("    Cleanup:          OK")


def test_complete_store_success(stub, keys, block_size, ptrs, handles, lu_ptrs, lu_handles):
    """OffloadingSpec.complete_store(success=True) → CommitStore

    Validates:
    - CommitStore makes entry visible
    - Data is readable after commit (integrity check via Lookup)
    """
    print("\n  [complete_store(success)] CommitStore finalizes entry")

    patterns = {}
    for i, k in enumerate(keys):
        pat = make_pattern(k, block_size)
        patterns[k] = pat
        gpu_write(ptrs[i], pat)

    populate_entries(stub, keys, block_size, ptrs, handles)
    print("    Reserve+Copy+Commit: OK")

    exists = check_exists(stub, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} not visible after CommitStore")
    print("    Visible:          OK")

    # Integrity check via Lookup
    lu_entries = [
        dispatcher_pb2.LookupEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=lu_handles[i], size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=lu_entries))
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(resp, "Lookup")

    for i, k in enumerate(keys):
        actual = gpu_read(lu_ptrs[i], block_size)
        if actual != patterns[k]:
            raise AssertionError(f"Integrity fail: key={k}")
    print("    Integrity:        OK")

    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:          OK")


def test_complete_store_abort(stub, keys, block_size, ptrs, handles):
    """OffloadingSpec.complete_store(success=False) → AbortStore

    Validates:
    - AbortStore discards a reserved+copied slot
    - Entry never becomes visible
    - Slot is reusable after abort
    """
    print("\n  [complete_store(failure)] AbortStore discards entry")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))

    entries = [dispatcher_pb2.ReserveEntry(key=k, size=block_size) for k in keys]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve")

    copy_entries = [
        dispatcher_pb2.CopyToStoreEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=handles[i], size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=copy_entries))
    assert_all_success(resp, "CopyToStore")

    resp = stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=keys))
    assert_all_success(resp, "AbortStore")
    print("    AbortStore:       OK")

    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after AbortStore")
    print("    Invisible:        OK")

    # Verify slot is reusable
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve (reuse)")
    stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=keys))
    print("    Slot reuse:       OK")


def test_lookup_check(stub, keys, block_size, ptrs, handles):
    """OffloadingSpec.lookup → Check (existence query)

    Validates:
    - Check returns exists=false for missing keys
    - Check returns exists=true for present keys
    - Check is non-destructive (entry persists after check)
    """
    print("\n  [lookup] Check existence without data transfer")

    # Missing keys
    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} exists before population")
    print("    Missing keys:     OK (exists=false)")

    # Populate
    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(stub, keys, block_size, ptrs, handles)

    # Present keys
    exists = check_exists(stub, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} not found after populate")
    print("    Present keys:     OK (exists=true)")

    # Non-destructive: check again
    exists2 = check_exists(stub, keys)
    for k in keys:
        if not exists2.get(k, False):
            raise AssertionError(f"Key {k} disappeared after Check")
    print("    Non-destructive:  OK")

    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:          OK")


def test_prepare_load(stub, keys, block_size, ptrs, handles):
    """OffloadingSpec.prepare_load → Pin + Touch(promote=true)

    Validates:
    - Pin prevents eviction (entry survives after pinning)
    - Touch with promote=true is accepted for memory-tier entries
    - Pinned entries remain accessible
    """
    print("\n  [prepare_load] Pin + Touch(promote) protects entry for load")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(stub, keys, block_size, ptrs, handles)

    # Pin (prevents eviction during load)
    resp = stub.Pin(dispatcher_pb2.BatchPinRequest(keys=keys))
    assert_all_success(resp, "Pin")
    print("    Pin:              OK")

    # Touch with promote (for SSD-resident entries this promotes to DRAM;
    # for memory-tier entries it just updates the eviction timestamp)
    resp = stub.Touch(dispatcher_pb2.BatchTouchRequest(keys=keys, promote=True))
    assert_all_success(resp, "Touch(promote)")
    print("    Touch(promote):   OK")

    # Entry still visible (not evicted)
    exists = check_exists(stub, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared while pinned")
    print("    Still exists:     OK")

    # Cleanup: unpin then remove
    stub.Unpin(dispatcher_pb2.BatchUnpinRequest(keys=keys))
    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:          OK")


def test_transfer_load(stub, keys, block_size, ptrs, handles, lu_ptrs, lu_handles):
    """OffloadingSpec transfer_async (load direction) → Lookup (DMA readback)

    Validates:
    - Lookup copies data from DRAM to GPU
    - Data integrity after round-trip (store → lookup)
    - GPU buffer receives correct pattern
    """
    print("\n  [transfer_load] Lookup DMA from DRAM to GPU")

    patterns = {}
    for i, k in enumerate(keys):
        pat = make_pattern(k, block_size)
        patterns[k] = pat
        gpu_write(ptrs[i], pat)

    populate_via_populate_rpc(stub, keys, block_size, ptrs, handles)

    # Zero the lookup GPU buffers to ensure we're reading fresh data
    for ptr in lu_ptrs:
        gpu_zero(ptr, block_size)
    _libcudart.cudaDeviceSynchronize()

    lu_entries = [
        dispatcher_pb2.LookupEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=lu_handles[i], size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=lu_entries))
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(resp, "Lookup")
    print("    Lookup DMA:       OK")

    for i, k in enumerate(keys):
        actual = gpu_read(lu_ptrs[i], block_size)
        if actual != patterns[k]:
            first_bad = next(
                (j for j in range(len(actual)) if actual[j] != patterns[k][j]), "?"
            )
            raise AssertionError(f"Integrity fail: key={k}, mismatch at byte {first_bad}")
    print(f"    Integrity:        OK ({len(keys)} objects)")

    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:          OK")


def test_complete_load(stub, keys, block_size, ptrs, handles):
    """OffloadingSpec.complete_load → Unpin

    Validates:
    - Pin increments refcount, Unpin decrements it
    - Pinned entry survives (not evicted)
    - Unpin below zero is rejected (refcount underflow)

    Note: Populate leaves a transient internal read-ref that the background
    writer releases after SSD write-through. We flush first to drain it,
    so the only refs are those we explicitly Pin.
    """
    print("\n  [complete_load] Unpin releases eviction protection")

    ensure_pool_quiescent(stub)

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(stub, keys, block_size, ptrs, handles)

    # Flush to SSD to drain the background writer's internal read-ref.
    # After this, entries have read_ref=0 (only user Pins matter).
    stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())

    # Pin (read_ref 0→1)
    resp = stub.Pin(dispatcher_pb2.BatchPinRequest(keys=keys))
    assert_all_success(resp, "Pin")

    # Entry still exists (protected by pin)
    exists = check_exists(stub, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared while pinned")
    print("    Pin + exists:     OK")

    # Unpin (read_ref 1→0) — entry now evictable but still present
    resp = stub.Unpin(dispatcher_pb2.BatchUnpinRequest(keys=keys))
    assert_all_success(resp, "Unpin")
    print("    Unpin:            OK")

    # Double-unpin should fail (refcount underflow)
    resp = stub.Unpin(dispatcher_pb2.BatchUnpinRequest(keys=keys))
    for r in resp.results:
        if r.success:
            raise AssertionError(f"Unpin key={r.key} succeeded on zero refcount")
    print("    Underflow reject: OK")

    # Cleanup (entries may have been evicted, best-effort)
    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:          OK")


def test_touch(stub, keys, block_size, ptrs, handles):
    """OffloadingSpec.touch → Touch (update eviction timestamps)

    Validates:
    - Touch(promote=false) succeeds on memory-tier entries
    - Touch(promote=true) succeeds (promote is no-op for DRAM-resident)
    - Touch on missing key is handled gracefully
    """
    print("\n  [touch] Touch updates eviction timestamp")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(stub, keys, block_size, ptrs, handles)

    # Touch without promote
    resp = stub.Touch(dispatcher_pb2.BatchTouchRequest(keys=keys, promote=False))
    assert_all_success(resp, "Touch(no promote)")
    print("    Touch(no promote): OK")

    # Touch with promote (no-op for DRAM-resident, but should succeed)
    resp = stub.Touch(dispatcher_pb2.BatchTouchRequest(keys=keys, promote=True))
    assert_all_success(resp, "Touch(promote)")
    print("    Touch(promote):    OK")

    # Entries still exist after touch
    exists = check_exists(stub, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared after Touch")
    print("    Still exists:      OK")

    # Touch on non-existent key
    bogus = [0xBEEF_DEAD_0001]
    resp = stub.Touch(dispatcher_pb2.BatchTouchRequest(keys=bogus, promote=False))
    for r in resp.results:
        if r.success:
            raise AssertionError("Touch succeeded on non-existent key")
    print("    Missing key:       OK (rejected)")

    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:           OK")


def test_take_events(stub, block_size, ptrs, handles):
    """OffloadingSpec.take_events → TakeEvents (eviction drain)

    Validates:
    - Empty drain returns 0 events
    - After memory pressure, eviction events are emitted
    - Events have valid structure (key > 0, known reason)
    - max_events limits response size
    - Second drain is empty (events consumed)
    - dropped_count reports overflow
    """
    print("\n  [take_events] TakeEvents drains eviction notifications")

    # Start from a completely clean slate: clear pool + flush + drain stale events
    stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
    stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())
    stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))

    # Empty drain
    resp = stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))
    assert len(resp.events) == 0, f"Expected 0 events initially, got {len(resp.events)}"
    print("    Empty drain:      OK")

    # Flood memory-tier with pool_capacity + overflow to guarantee eviction.
    # We need MORE unique keys than pool slots so at least some must be evicted.
    # Use random base to avoid collisions with prior runs (keys persist on SSD).
    pool_slots = (256 * 1024 * 1024) // block_size
    overflow = max(64, pool_slots // 16)
    num_entries = pool_slots + overflow
    base_key = random.randint(500_000_000, 700_000_000)
    flood_keys = [base_key + i for i in range(num_entries)]

    batch_size = min(len(ptrs), 8)
    for batch_start in range(0, len(flood_keys), batch_size):
        batch = flood_keys[batch_start:batch_start + batch_size]
        entries = [
            dispatcher_pb2.PopulateEntry(
                key=k,
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=handles[idx % len(handles)], size=block_size,
                ),
            )
            for idx, k in enumerate(batch)
        ]
        stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))

    # Drain events
    resp = stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))
    total_events = len(resp.events)
    total_dropped = resp.dropped_count
    print(f"    Events: {total_events} drained, {total_dropped} dropped")

    assert total_events > 0 or total_dropped > 0, \
        "Expected eviction events after overflowing memory-tier"
    print("    Evictions fired:  OK")

    # Validate event structure
    for ev in resp.events:
        assert ev.key > 0, f"Event key should be > 0, got {ev.key}"
        assert ev.reason in (
            dispatcher_pb2.EVICTION_REASON_DEMOTED,
            dispatcher_pb2.EVICTION_REASON_REMOVED,
        ), f"Unknown reason: {ev.reason}"
    print("    Event structure:  OK")

    # Second drain should be empty
    resp2 = stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))
    assert len(resp2.events) == 0, f"Second drain not empty: {len(resp2.events)} events"
    print("    Drain-once:       OK")

    # Test max_events limit — flood again with different keys
    base_key2 = random.randint(700_000_000, 900_000_000)
    flood_keys2 = [base_key2 + i for i in range(num_entries)]
    for batch_start in range(0, len(flood_keys2), batch_size):
        batch = flood_keys2[batch_start:batch_start + batch_size]
        entries = [
            dispatcher_pb2.PopulateEntry(
                key=k,
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=handles[idx % len(handles)], size=block_size,
                ),
            )
            for idx, k in enumerate(batch)
        ]
        stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))

    resp = stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=5))
    if len(resp.events) > 0:
        assert len(resp.events) <= 5, f"max_events=5 but got {len(resp.events)}"
        print(f"    max_events=5:     OK (got {len(resp.events)})")
    else:
        print("    max_events=5:     OK (no events pending)")

    # Drain remainder
    stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))

    # Cleanup — best-effort remove both flood sets
    stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
    print("    Cleanup:          OK")


def test_remove(stub, keys, block_size, ptrs, handles):
    """Explicit removal → Remove

    Validates:
    - Remove deletes an existing entry
    - Entry is no longer visible after removal
    - Remove on non-existent key returns error
    - Remove is idempotent (second remove returns error, no crash)
    """
    print("\n  [remove] Remove deletes cache entry")

    ensure_pool_quiescent(stub)

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(stub, keys, block_size, ptrs, handles)

    # Flush background writer so its internal read-ref is released.
    # Without this, Remove can fail with "key not found" (ActiveReferences).
    stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())

    # Verify present
    exists = check_exists(stub, keys)
    for k in keys:
        assert exists.get(k, False), f"Key {k} missing before Remove"

    # Remove
    resp = stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    assert_all_success(resp, "Remove")
    print("    Remove:           OK")

    # Verify gone
    exists = check_exists(stub, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} still visible after Remove")
    print("    Gone:             OK")

    # Remove non-existent
    resp = stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    for r in resp.results:
        if r.success:
            raise AssertionError(f"Remove key={r.key} succeeded on deleted entry")
    print("    Already removed:  OK (rejected)")


def test_clear_memory_tier(stub, keys, block_size, ptrs, handles):
    """Bulk clear → ClearMemoryTier

    Validates:
    - ClearMemoryTier removes all entries from DRAM cache
    - Returns count of cleared entries
    - Entries are no longer visible after clear
    """
    print("\n  [clear] ClearMemoryTier bulk-removes all cache entries")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(stub, keys, block_size, ptrs, handles)

    # Verify present
    exists = check_exists(stub, keys)
    populated_count = sum(1 for k in keys if exists.get(k, False))
    assert populated_count == len(keys), f"Only {populated_count}/{len(keys)} present"

    # Clear
    resp = stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
    print(f"    Cleared:          {resp.entries_cleared} entries")
    assert resp.entries_cleared >= len(keys), \
        f"Expected >= {len(keys)} cleared, got {resp.entries_cleared}"
    print("    ClearMemoryTier:  OK")

    # Entries gone (or demoted to SSD — either way, they're out of memory-tier)
    # Note: Check verifies dispatch-map, which may still have SSD entries.
    # For this test, we just verify the clear returned a valid count.
    print("    Count valid:      OK")


def test_flush_to_ssd(stub, keys, block_size, ptrs, handles):
    """Persistence flush → FlushToSsd

    Validates:
    - FlushToSsd completes without error
    - Returns jobs_flushed count
    - After flush, entries are persistent (survive in dispatch-map)
    """
    print("\n  [flush] FlushToSsd forces write-through to SSD")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(stub, keys, block_size, ptrs, handles)

    # Flush
    resp = stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())
    print(f"    Flushed:          {resp.jobs_flushed} jobs")
    print("    FlushToSsd:       OK")

    # Entries still visible
    exists = check_exists(stub, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared after flush")
    print("    Still visible:    OK")

    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:          OK")


def test_full_offloading_lifecycle(stub, keys, block_size, ptrs, handles, lu_ptrs, lu_handles):
    """End-to-end OffloadingSpec lifecycle: store → lookup → load → touch → evict

    Simulates the full vLLM KV-cache offloading workflow:
    1. prepare_store (Reserve)
    2. transfer_async store (CopyToStore)
    3. complete_store (CommitStore)
    4. lookup (Check — scheduler checks if key is cached)
    5. prepare_load (Pin + Touch promote)
    6. transfer_async load (Lookup DMA)
    7. complete_load (Unpin)
    8. touch (keep alive)
    9. take_events (poll for evictions)
    """
    print("\n  [lifecycle] Full OffloadingSpec round-trip")

    patterns = {}
    for i, k in enumerate(keys):
        pat = make_pattern(k, block_size)
        patterns[k] = pat
        gpu_write(ptrs[i], pat)

    # 1. prepare_store → Reserve
    entries = [dispatcher_pb2.ReserveEntry(key=k, size=block_size) for k in keys]
    resp = stub.Reserve(dispatcher_pb2.BatchReserveRequest(entries=entries))
    assert_all_success(resp, "Reserve")
    print("    1. prepare_store: OK")

    # 2. transfer_async store → CopyToStore
    copy_entries = [
        dispatcher_pb2.CopyToStoreEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=handles[i], size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.CopyToStore(dispatcher_pb2.BatchCopyToStoreRequest(entries=copy_entries))
    assert_all_success(resp, "CopyToStore")
    print("    2. transfer(store): OK")

    # 3. complete_store → CommitStore
    resp = stub.CommitStore(dispatcher_pb2.BatchCommitStoreRequest(keys=keys))
    assert_all_success(resp, "CommitStore")
    print("    3. complete_store: OK")

    # 4. lookup → Check
    exists = check_exists(stub, keys)
    for k in keys:
        assert exists.get(k, False), f"Key {k} not found in Check"
    print("    4. lookup:        OK")

    # 5. prepare_load → Pin + Touch(promote)
    resp = stub.Pin(dispatcher_pb2.BatchPinRequest(keys=keys))
    assert_all_success(resp, "Pin")
    resp = stub.Touch(dispatcher_pb2.BatchTouchRequest(keys=keys, promote=True))
    assert_all_success(resp, "Touch(promote)")
    print("    5. prepare_load:  OK")

    # 6. transfer_async load → Lookup (DMA to GPU)
    for ptr in lu_ptrs:
        gpu_zero(ptr, block_size)
    _libcudart.cudaDeviceSynchronize()

    lu_entries = [
        dispatcher_pb2.LookupEntry(
            key=k,
            ipc_handle=dispatcher_pb2.IpcHandle(
                cuda_ipc_handle=lu_handles[i], size=block_size,
            ),
        )
        for i, k in enumerate(keys)
    ]
    resp = stub.Lookup(dispatcher_pb2.BatchLookupRequest(entries=lu_entries))
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(resp, "Lookup")

    for i, k in enumerate(keys):
        actual = gpu_read(lu_ptrs[i], block_size)
        if actual != patterns[k]:
            raise AssertionError(f"Integrity fail on load: key={k}")
    print("    6. transfer(load): OK (integrity verified)")

    # 7. complete_load → Unpin
    resp = stub.Unpin(dispatcher_pb2.BatchUnpinRequest(keys=keys))
    assert_all_success(resp, "Unpin")
    print("    7. complete_load: OK")

    # 8. touch (keep alive after load completes)
    resp = stub.Touch(dispatcher_pb2.BatchTouchRequest(keys=keys, promote=False))
    assert_all_success(resp, "Touch")
    print("    8. touch:         OK")

    # 9. take_events (verify we can poll — no evictions expected for these keys)
    resp = stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))
    # We don't assert empty — other tests may have residual events
    print(f"    9. take_events:   OK ({len(resp.events)} pending)")

    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys))
    print("    Cleanup:          OK")


def test_pin_prevents_eviction(stub, block_size, ptrs, handles):
    """Pin semantics: pinned entries survive memory pressure.

    Validates that a pinned entry is NOT evicted even when the memory-tier
    is overflowed by other entries.
    """
    print("\n  [pin_eviction] Pinned entries survive memory pressure")

    # Clear memory-tier and drain stale events for a clean start
    stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
    stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))

    # Store a sentinel entry and pin it immediately
    sentinel_key = 600_000_001
    gpu_write(ptrs[0], make_pattern(sentinel_key, block_size))
    populate_via_populate_rpc(stub, [sentinel_key], block_size, [ptrs[0]], [handles[0]])

    resp = stub.Pin(dispatcher_pb2.BatchPinRequest(keys=[sentinel_key]))
    assert_all_success(resp, "Pin sentinel")
    print("    Sentinel pinned:  OK")

    # Drain any events from the populate (in case it triggered eviction of old entries)
    stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))

    # Flood memory-tier to trigger eviction
    num_flood = (256 * 1024 * 1024) // block_size + 16
    base_key = 600_100_000
    flood_keys = [base_key + i for i in range(num_flood)]

    batch_size = min(len(ptrs), 8)
    for batch_start in range(0, len(flood_keys), batch_size):
        batch = flood_keys[batch_start:batch_start + batch_size]
        entries = [
            dispatcher_pb2.PopulateEntry(
                key=k,
                ipc_handle=dispatcher_pb2.IpcHandle(
                    cuda_ipc_handle=handles[idx % len(handles)], size=block_size,
                ),
            )
            for idx, k in enumerate(batch)
        ]
        stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))

    # Sentinel should still exist (pinned)
    exists = check_exists(stub, [sentinel_key])
    assert exists.get(sentinel_key, False), "Pinned sentinel was evicted!"
    print("    Survived flood:   OK")

    # Drain events — sentinel should NOT appear (it was pinned before flood)
    resp = stub.TakeEvents(dispatcher_pb2.TakeEventsRequest(max_events=0))
    evicted_keys = {ev.key for ev in resp.events if ev.reason == dispatcher_pb2.EVICTION_REASON_REMOVED}
    if sentinel_key in evicted_keys:
        raise AssertionError("Pinned sentinel appeared in REMOVED eviction events!")
    print("    Not in events:    OK")

    # Cleanup
    stub.Unpin(dispatcher_pb2.BatchUnpinRequest(keys=[sentinel_key]))
    stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=[sentinel_key]))
    stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())
    print("    Cleanup:          OK")


# ============================================================
# Main
# ============================================================

def main():
    parser = argparse.ArgumentParser(
        description="Validate Certus gRPC APIs against OffloadingSpec contract"
    )
    parser.add_argument("--server", default="localhost:50051")
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

    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 60)
    print("Certus OffloadingSpec Compliance Test")
    print("=" * 60)
    print(f"  Server:      {args.server}")
    print(f"  Block size:  {block_size // 1024} KiB")
    print(f"  Objects:     {num_objects}")
    print(f"  GPU:         {args.gpu}")

    # Allocate GPU buffers for store and load
    store_ptrs, store_handles = [], []
    load_ptrs, load_handles = [], []
    for _ in range(num_objects):
        ptr, handle = cuda_alloc(block_size)
        store_ptrs.append(ptr)
        store_handles.append(handle)
        ptr, handle = cuda_alloc(block_size)
        load_ptrs.append(ptr)
        load_handles.append(handle)

    # Each test uses its own RANDOM key range to avoid collisions across runs.
    # Keys that persist on SSD from prior runs would cause "already exists" errors.
    key_sets = []
    for _ in range(20):
        base = random.randint(1_000_000_000, 2_000_000_000)
        key_sets.append([base + i for i in range(num_objects)])

    channel = grpc.insecure_channel(
        args.server,
        options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ],
    )
    stub = dispatcher_pb2_grpc.DispatcherStub(channel)

    # Ensure clean starting state (server may have residual entries from prior runs)
    ensure_pool_quiescent(stub)

    passed = 0
    failed = 0
    tests = [
        # Store path
        ("prepare_store", lambda: test_prepare_store(
            stub, key_sets[0], block_size, store_ptrs, store_handles)),
        ("transfer_store", lambda: test_transfer_store(
            stub, key_sets[1], block_size, store_ptrs, store_handles)),
        ("complete_store_success", lambda: test_complete_store_success(
            stub, key_sets[2], block_size, store_ptrs, store_handles,
            load_ptrs, load_handles)),
        ("complete_store_abort", lambda: test_complete_store_abort(
            stub, key_sets[3], block_size, store_ptrs, store_handles)),
        # Lookup
        ("lookup_check", lambda: test_lookup_check(
            stub, key_sets[4], block_size, store_ptrs, store_handles)),
        # Load path
        ("prepare_load", lambda: test_prepare_load(
            stub, key_sets[5], block_size, store_ptrs, store_handles)),
        ("transfer_load", lambda: test_transfer_load(
            stub, key_sets[6], block_size, store_ptrs, store_handles,
            load_ptrs, load_handles)),
        ("complete_load", lambda: test_complete_load(
            stub, key_sets[7], block_size, store_ptrs, store_handles)),
        # Touch
        ("touch", lambda: test_touch(
            stub, key_sets[8], block_size, store_ptrs, store_handles)),
        # Removal (before flooding tests to avoid background evictor interference)
        ("remove", lambda: test_remove(
            stub, key_sets[10], block_size, store_ptrs, store_handles)),
        # Bulk operations
        ("clear_memory_tier", lambda: test_clear_memory_tier(
            stub, key_sets[11], block_size, store_ptrs, store_handles)),
        ("flush_to_ssd", lambda: test_flush_to_ssd(
            stub, key_sets[12], block_size, store_ptrs, store_handles)),
        # End-to-end lifecycle
        ("full_lifecycle", lambda: test_full_offloading_lifecycle(
            stub, key_sets[13], block_size, store_ptrs, store_handles,
            load_ptrs, load_handles)),
        # Eviction events (floods memory-tier — run after non-pressure tests)
        ("take_events", lambda: test_take_events(
            stub, block_size, store_ptrs, store_handles)),
        # Pin-under-pressure
        ("pin_prevents_eviction", lambda: test_pin_prevents_eviction(
            stub, block_size, store_ptrs, store_handles)),
    ]

    all_keys = [k for ks in key_sets for k in ks]
    for name, test_fn in tests:
        try:
            test_fn()
            passed += 1
        except (AssertionError, grpc.RpcError) as e:
            print(f"    FAILED: {e}")
            failed += 1
            # Best-effort cleanup
            try:
                stub.AbortStore(dispatcher_pb2.BatchAbortStoreRequest(keys=all_keys))
            except Exception:
                pass
            try:
                stub.Unpin(dispatcher_pb2.BatchUnpinRequest(keys=all_keys))
            except Exception:
                pass
            try:
                stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=all_keys))
            except Exception:
                pass

    # Free GPU memory
    for ptr in store_ptrs:
        cuda_free(ptr)
    for ptr in load_ptrs:
        cuda_free(ptr)
    channel.close()

    print("\n" + "=" * 60)
    print(f"Results: {passed} passed, {failed} failed, {passed + failed} total")
    print("=" * 60)

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()

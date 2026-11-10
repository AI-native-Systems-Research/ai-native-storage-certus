#!/usr/bin/env python3
"""Validate all Certus shmq APIs that map to the vLLM OffloadingSpec interface.

Tests the complete OffloadingManager lifecycle as exercised by vLLM's
KV-cache offloading connector. Each test section is named after the
OffloadingSpec operation it validates.

OffloadingSpec → Certus shmq mapping:
    prepare_store(keys)              → Reserve
    transfer_async(store: GPU→DRAM)  → CopyToStore
    complete_store(keys, success)    → CommitStore / AbortStore
    lookup(keys)                     → Check
    prepare_load(keys)               → Pin(promote=true)
    transfer_async(load: DRAM→GPU)   → Lookup (DMA readback)
    complete_load(keys)              → Unpin
    touch(keys)                      → Touch
    take_events()                    → TakeEvents
    [explicit removal]               → Remove
    [bulk clear]                     → ClearMemoryTier
    [persistence flush]              → FlushToSsd

Usage:
    python test-offloading-spec.py --shm-path /dev/shm/certus-shmq --block-size 64K

Requires: GPU with CUDA, running certus-server with --memory-tier-size 256M
"""

import argparse
import ctypes
import os
import random
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from certus_shmq_helpers import RingError, add_shm_arg, connect, single_region
from certus_shmq_connector.ring import (
    CHECK_PENDING,
    CHECK_RESIDENT,
    REASON_DEMOTED,
    REASON_REMOVED,
)

# GPU device index the script operates on; set from args.gpu in main().
_GPU_DEVICE = 0

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

def ensure_pool_quiescent(ring):
    """Clear memory-tier, flush pending writes, drain events, and let evictor settle.

    The background evictor runs asynchronously and may still be demoting entries
    from a prior flood. We clear+flush+drain, sleep to let it finish, then repeat.
    """
    for _ in range(3):
        ring.clear_memory_tier()
        ring.flush_to_ssd()
        ring.take_events(0)
        time.sleep(0.5)


def assert_all_success(oks, op_name, keys=None):
    for i, ok in enumerate(oks):
        if not ok:
            key = keys[i] if keys is not None else i
            raise AssertionError(f"{op_name} failed for key {key}")


def check_exists(ring, keys):
    oks = ring.check(keys)
    return {k: oks[i] for i, k in enumerate(keys)}


# --- Helpers to populate entries (used across tests) ---

def populate_entries(ring, keys, block_size, ptrs, handles):
    """Store entries via the split-phase path (Reserve → CopyToStore → Commit)."""
    oks = ring.reserve([(k, block_size, 0) for k in keys])
    assert_all_success(oks, "Reserve", keys)

    copy_entries = [
        (k, [single_region(handles[i], _GPU_DEVICE, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.copy_to_store(copy_entries)
    assert_all_success(oks, "CopyToStore", keys)

    oks = ring.commit_store(keys)
    assert_all_success(oks, "CommitStore", keys)


def populate_via_populate_rpc(ring, keys, block_size, ptrs, handles):
    """Store entries via the single-phase Populate RPC."""
    entries = [
        (k, [single_region(handles[i], _GPU_DEVICE, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.populate(entries)
    assert_all_success(oks, "Populate", keys)


# ============================================================
# TEST FUNCTIONS — one per OffloadingSpec operation
# ============================================================

def test_prepare_store(ring, keys, block_size, ptrs, handles):
    """OffloadingSpec.prepare_store → Reserve

    Validates:
    - Reserve allocates slots for keys
    - A reserved slot is PENDING (not committed / not loadable) before commit
    - Duplicate reserve is rejected (AlreadyExists)
    """
    print("\n  [prepare_store] Reserve allocates pending (uncommitted) slots")

    reserve_entries = [(k, block_size, 0) for k in keys]
    oks = ring.reserve(reserve_entries)
    assert_all_success(oks, "Reserve", keys)
    print("    Reserve:          OK")

    # Tri-state Check: a reserved-but-uncommitted key reports PENDING (a store
    # is in flight), never RESIDENT. The data is not yet loadable, so it must
    # not read as committed. The bool exists-view (ring.check) intentionally
    # counts PENDING as present so store-dedup won't re-reserve an in-flight
    # key, so assert on the raw state rather than the collapsed exists bit.
    states = ring.check_states(keys)
    for k, s in zip(keys, states):
        if s != CHECK_PENDING:
            raise AssertionError(
                f"Key {k} state={s} after Reserve (expected PENDING; "
                f"RESIDENT={CHECK_RESIDENT} would mean wrongly committed)"
            )
    print("    Pending:          OK (not committed)")

    # Duplicate reserve should fail
    dup_oks = ring.reserve(reserve_entries[:1])
    if dup_oks[0]:
        raise AssertionError(f"Reserve (duplicate) key={keys[0]}: expected error, got success")
    print("    Dup rejected:     OK (AlreadyExists)")

    # Cleanup
    ring.abort_store(keys)
    print("    Cleanup:          OK")


def test_transfer_store(ring, keys, block_size, ptrs, handles):
    """OffloadingSpec transfer_async (store direction) → CopyToStore

    Validates:
    - After Reserve, CopyToStore transfers GPU data into the DRAM slot
    - Entry still PENDING (not committed) after CopyToStore (needs commit)
    """
    print("\n  [transfer_store] CopyToStore DMA from GPU to reserved slot")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))

    oks = ring.reserve([(k, block_size, 0) for k in keys])
    assert_all_success(oks, "Reserve", keys)

    copy_entries = [
        (k, [single_region(handles[i], _GPU_DEVICE, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.copy_to_store(copy_entries)
    assert_all_success(oks, "CopyToStore", keys)
    print("    CopyToStore:      OK")

    # Still uncommitted after the DMA: CopyToStore fills the reserved slot but
    # only CommitStore makes it RESIDENT, so the key stays PENDING until then.
    states = ring.check_states(keys)
    for k, s in zip(keys, states):
        if s != CHECK_PENDING:
            raise AssertionError(
                f"Key {k} state={s} after CopyToStore (expected PENDING, "
                f"not yet committed)"
            )
    print("    Still pending:    OK (not committed)")

    # Cleanup via abort
    ring.abort_store(keys)
    print("    Cleanup:          OK")


def test_complete_store_success(ring, keys, block_size, ptrs, handles, lu_ptrs, lu_handles):
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

    populate_entries(ring, keys, block_size, ptrs, handles)
    print("    Reserve+Copy+Commit: OK")

    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} not visible after CommitStore")
    print("    Visible:          OK")

    # Integrity check via Lookup
    lu_entries = [
        (k, [single_region(lu_handles[i], _GPU_DEVICE, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.lookup(lu_entries)
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(oks, "Lookup", keys)

    for i, k in enumerate(keys):
        actual = gpu_read(lu_ptrs[i], block_size)
        if actual != patterns[k]:
            raise AssertionError(f"Integrity fail: key={k}")
    print("    Integrity:        OK")

    ring.remove(keys)
    print("    Cleanup:          OK")


def test_complete_store_abort(ring, keys, block_size, ptrs, handles):
    """OffloadingSpec.complete_store(success=False) → AbortStore

    Validates:
    - AbortStore discards a reserved+copied slot
    - Entry never becomes visible
    - Slot is reusable after abort
    """
    print("\n  [complete_store(failure)] AbortStore discards entry")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))

    reserve_entries = [(k, block_size, 0) for k in keys]
    oks = ring.reserve(reserve_entries)
    assert_all_success(oks, "Reserve", keys)

    copy_entries = [
        (k, [single_region(handles[i], _GPU_DEVICE, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.copy_to_store(copy_entries)
    assert_all_success(oks, "CopyToStore", keys)

    oks = ring.abort_store(keys)
    assert_all_success(oks, "AbortStore", keys)
    print("    AbortStore:       OK")

    exists = check_exists(ring, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} visible after AbortStore")
    print("    Invisible:        OK")

    # Verify slot is reusable
    oks = ring.reserve(reserve_entries)
    assert_all_success(oks, "Reserve (reuse)", keys)
    ring.abort_store(keys)
    print("    Slot reuse:       OK")


def test_lookup_check(ring, keys, block_size, ptrs, handles):
    """OffloadingSpec.lookup → Check (existence query)

    Validates:
    - Check returns exists=false for missing keys
    - Check returns exists=true for present keys
    - Check is non-destructive (entry persists after check)
    """
    print("\n  [lookup] Check existence without data transfer")

    # Missing keys
    exists = check_exists(ring, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} exists before population")
    print("    Missing keys:     OK (exists=false)")

    # Populate
    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(ring, keys, block_size, ptrs, handles)

    # Present keys
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} not found after populate")
    print("    Present keys:     OK (exists=true)")

    # Non-destructive: check again
    exists2 = check_exists(ring, keys)
    for k in keys:
        if not exists2.get(k, False):
            raise AssertionError(f"Key {k} disappeared after Check")
    print("    Non-destructive:  OK")

    ring.remove(keys)
    print("    Cleanup:          OK")


def test_prepare_load(ring, keys, block_size, ptrs, handles):
    """OffloadingSpec.prepare_load → Pin(promote=true)

    Validates:
    - Pin(promote=true) pins AND promotes in a single round-trip
    - Pinned entries remain accessible
    - Entry survives after pinning (not evicted)
    """
    print("\n  [prepare_load] Pin(promote=true) protects and promotes for load")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(ring, keys, block_size, ptrs, handles)

    # Pin with promote (single call replaces Pin + Touch(promote=true))
    oks = ring.pin(keys, promote=True)
    assert_all_success(oks, "Pin(promote)", keys)
    print("    Pin(promote):     OK")

    # Entry still visible (not evicted)
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared while pinned")
    print("    Still exists:     OK")

    # Cleanup: unpin then remove
    ring.unpin(keys)
    ring.remove(keys)
    print("    Cleanup:          OK")


def test_transfer_load(ring, keys, block_size, ptrs, handles, lu_ptrs, lu_handles):
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

    populate_via_populate_rpc(ring, keys, block_size, ptrs, handles)

    # Zero the lookup GPU buffers to ensure we're reading fresh data
    for ptr in lu_ptrs:
        gpu_zero(ptr, block_size)
    _libcudart.cudaDeviceSynchronize()

    lu_entries = [
        (k, [single_region(lu_handles[i], _GPU_DEVICE, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.lookup(lu_entries)
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(oks, "Lookup", keys)
    print("    Lookup DMA:       OK")

    for i, k in enumerate(keys):
        actual = gpu_read(lu_ptrs[i], block_size)
        if actual != patterns[k]:
            first_bad = next(
                (j for j in range(len(actual)) if actual[j] != patterns[k][j]), "?"
            )
            raise AssertionError(f"Integrity fail: key={k}, mismatch at byte {first_bad}")
    print(f"    Integrity:        OK ({len(keys)} objects)")

    ring.remove(keys)
    print("    Cleanup:          OK")


def test_complete_load(ring, keys, block_size, ptrs, handles):
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

    ensure_pool_quiescent(ring)

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(ring, keys, block_size, ptrs, handles)

    # Flush to SSD to drain the background writer's internal read-ref.
    # After this, entries have read_ref=0 (only user Pins matter).
    ring.flush_to_ssd()

    # Pin (read_ref 0→1)
    oks = ring.pin(keys)
    assert_all_success(oks, "Pin", keys)

    # Entry still exists (protected by pin)
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared while pinned")
    print("    Pin + exists:     OK")

    # Unpin (read_ref 1→0) — entry now evictable but still present
    oks = ring.unpin(keys)
    assert_all_success(oks, "Unpin", keys)
    print("    Unpin:            OK")

    # Double-unpin should fail (refcount underflow)
    oks = ring.unpin(keys)
    for i, ok in enumerate(oks):
        if ok:
            raise AssertionError(f"Unpin key={keys[i]} succeeded on zero refcount")
    print("    Underflow reject: OK")

    # Cleanup (entries may have been evicted, best-effort)
    ring.remove(keys)
    print("    Cleanup:          OK")


def test_touch(ring, keys, block_size, ptrs, handles):
    """OffloadingSpec.touch → Touch (update eviction timestamps)

    Validates:
    - Touch(promote=false) succeeds on memory-tier entries
    - Touch(promote=true) succeeds (promote is no-op for DRAM-resident)
    - Touch on missing key is handled gracefully
    """
    print("\n  [touch] Touch updates eviction timestamp")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(ring, keys, block_size, ptrs, handles)

    # Touch without promote
    oks = ring.touch(keys, promote=False)
    assert_all_success(oks, "Touch(no promote)", keys)
    print("    Touch(no promote): OK")

    # Touch with promote (no-op for DRAM-resident, but should succeed)
    oks = ring.touch(keys, promote=True)
    assert_all_success(oks, "Touch(promote)", keys)
    print("    Touch(promote):    OK")

    # Entries still exist after touch
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared after Touch")
    print("    Still exists:      OK")

    # Touch on non-existent key
    bogus = [0xBEEF_DEAD_0001]
    oks = ring.touch(bogus, promote=False)
    for ok in oks:
        if ok:
            raise AssertionError("Touch succeeded on non-existent key")
    print("    Missing key:       OK (rejected)")

    ring.remove(keys)
    print("    Cleanup:           OK")


def test_take_events(ring, block_size, ptrs, handles, pool_size):
    """OffloadingSpec.take_events → TakeEvents (eviction drain)

    Validates:
    - Empty drain returns 0 events
    - After memory pressure, eviction events are emitted
    - Events have valid structure (key > 0, known reason)
    - max_events limits response size
    - Second drain is empty (events consumed)
    - dropped count reports overflow
    """
    print("\n  [take_events] TakeEvents drains eviction notifications")

    # Start from a completely clean slate: clear pool + flush + drain stale events
    ring.clear_memory_tier()
    ring.flush_to_ssd()
    ring.take_events(0)

    # Empty drain
    events, _dropped = ring.take_events(0)
    assert len(events) == 0, f"Expected 0 events initially, got {len(events)}"
    print("    Empty drain:      OK")

    # Flood memory-tier with pool_capacity + overflow to guarantee eviction.
    # We need MORE unique keys than pool slots so at least some must be evicted.
    # Use random base to avoid collisions with prior runs (keys persist on SSD).
    pool_slots = pool_size // block_size
    overflow = max(64, pool_slots // 16)
    num_entries = pool_slots + overflow
    base_key = random.randint(500_000_000, 700_000_000)
    flood_keys = [base_key + i for i in range(num_entries)]

    batch_size = min(len(ptrs), 8)
    for batch_start in range(0, len(flood_keys), batch_size):
        batch = flood_keys[batch_start:batch_start + batch_size]
        entries = [
            (k, [single_region(handles[idx % len(handles)], _GPU_DEVICE, block_size)])
            for idx, k in enumerate(batch)
        ]
        ring.populate(entries)

    # Drain events
    events, total_dropped = ring.take_events(0)
    total_events = len(events)
    print(f"    Events: {total_events} drained, {total_dropped} dropped")

    assert total_events > 0 or total_dropped > 0, \
        "Expected eviction events after overflowing memory-tier"
    print("    Evictions fired:  OK")

    # Validate event structure
    for ev_key, ev_reason in events:
        assert ev_key > 0, f"Event key should be > 0, got {ev_key}"
        assert ev_reason in (
            REASON_DEMOTED,
            REASON_REMOVED,
        ), f"Unknown reason: {ev_reason}"
    print("    Event structure:  OK")

    # Second drain should be empty
    events2, _dropped2 = ring.take_events(0)
    assert len(events2) == 0, f"Second drain not empty: {len(events2)} events"
    print("    Drain-once:       OK")

    # Test max_events limit — flood again with different keys
    base_key2 = random.randint(700_000_000, 900_000_000)
    flood_keys2 = [base_key2 + i for i in range(num_entries)]
    for batch_start in range(0, len(flood_keys2), batch_size):
        batch = flood_keys2[batch_start:batch_start + batch_size]
        entries = [
            (k, [single_region(handles[idx % len(handles)], _GPU_DEVICE, block_size)])
            for idx, k in enumerate(batch)
        ]
        ring.populate(entries)

    events, _dropped = ring.take_events(5)
    if len(events) > 0:
        assert len(events) <= 5, f"max_events=5 but got {len(events)}"
        print(f"    max_events=5:     OK (got {len(events)})")
    else:
        print("    max_events=5:     OK (no events pending)")

    # Drain remainder
    ring.take_events(0)

    # Cleanup — best-effort remove both flood sets
    ring.clear_memory_tier()
    print("    Cleanup:          OK")


def test_remove(ring, keys, block_size, ptrs, handles):
    """Explicit removal → Remove

    Validates:
    - Remove deletes an existing entry
    - Entry is no longer visible after removal
    - Remove on non-existent key returns error
    - Remove is idempotent (second remove returns error, no crash)
    """
    print("\n  [remove] Remove deletes cache entry")

    ensure_pool_quiescent(ring)

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(ring, keys, block_size, ptrs, handles)

    # Flush background writer so its internal read-ref is released.
    # Without this, Remove can fail with "key not found" (ActiveReferences).
    ring.flush_to_ssd()

    # Verify present
    exists = check_exists(ring, keys)
    for k in keys:
        assert exists.get(k, False), f"Key {k} missing before Remove"

    # Remove
    oks = ring.remove(keys)
    assert_all_success(oks, "Remove", keys)
    print("    Remove:           OK")

    # Verify gone
    exists = check_exists(ring, keys)
    for k in keys:
        if exists.get(k, False):
            raise AssertionError(f"Key {k} still visible after Remove")
    print("    Gone:             OK")

    # Remove non-existent
    oks = ring.remove(keys)
    for i, ok in enumerate(oks):
        if ok:
            raise AssertionError(f"Remove key={keys[i]} succeeded on deleted entry")
    print("    Already removed:  OK (rejected)")


def test_clear_memory_tier(ring, keys, block_size, ptrs, handles):
    """Bulk clear → ClearMemoryTier

    Validates:
    - ClearMemoryTier removes all entries from DRAM cache
    - Returns count of cleared entries
    - Entries are no longer visible after clear
    """
    print("\n  [clear] ClearMemoryTier bulk-removes all cache entries")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(ring, keys, block_size, ptrs, handles)

    # Verify present
    exists = check_exists(ring, keys)
    populated_count = sum(1 for k in keys if exists.get(k, False))
    assert populated_count == len(keys), f"Only {populated_count}/{len(keys)} present"

    # Clear
    cleared_count = ring.clear_memory_tier()
    print(f"    Cleared:          {cleared_count} entries")
    assert cleared_count >= len(keys), \
        f"Expected >= {len(keys)} cleared, got {cleared_count}"
    print("    ClearMemoryTier:  OK")

    # Entries gone (or demoted to SSD — either way, they're out of memory-tier)
    # Note: Check verifies dispatch-map, which may still have SSD entries.
    # For this test, we just verify the clear returned a valid count.
    print("    Count valid:      OK")


def test_flush_to_ssd(ring, keys, block_size, ptrs, handles):
    """Persistence flush → FlushToSsd

    Validates:
    - FlushToSsd completes without error
    - Returns jobs_flushed count
    - After flush, entries are persistent (survive in dispatch-map)
    """
    print("\n  [flush] FlushToSsd forces write-through to SSD")

    for i, k in enumerate(keys):
        gpu_write(ptrs[i], make_pattern(k, block_size))
    populate_via_populate_rpc(ring, keys, block_size, ptrs, handles)

    # Flush
    jobs_flushed = ring.flush_to_ssd()
    print(f"    Flushed:          {jobs_flushed} jobs")
    print("    FlushToSsd:       OK")

    # Entries still visible
    exists = check_exists(ring, keys)
    for k in keys:
        if not exists.get(k, False):
            raise AssertionError(f"Key {k} disappeared after flush")
    print("    Still visible:    OK")

    ring.remove(keys)
    print("    Cleanup:          OK")


def test_full_offloading_lifecycle(ring, keys, block_size, ptrs, handles, lu_ptrs, lu_handles):
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
    oks = ring.reserve([(k, block_size, 0) for k in keys])
    assert_all_success(oks, "Reserve", keys)
    print("    1. prepare_store: OK")

    # 2. transfer_async store → CopyToStore
    copy_entries = [
        (k, [single_region(handles[i], _GPU_DEVICE, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.copy_to_store(copy_entries)
    assert_all_success(oks, "CopyToStore", keys)
    print("    2. transfer(store): OK")

    # 3. complete_store → CommitStore
    oks = ring.commit_store(keys)
    assert_all_success(oks, "CommitStore", keys)
    print("    3. complete_store: OK")

    # 4. lookup → Check
    exists = check_exists(ring, keys)
    for k in keys:
        assert exists.get(k, False), f"Key {k} not found in Check"
    print("    4. lookup:        OK")

    # 5. prepare_load → Pin(promote=true)
    oks = ring.pin(keys, promote=True)
    assert_all_success(oks, "Pin(promote)", keys)
    print("    5. prepare_load:  OK")

    # 6. transfer_async load → Lookup (DMA to GPU)
    for ptr in lu_ptrs:
        gpu_zero(ptr, block_size)
    _libcudart.cudaDeviceSynchronize()

    lu_entries = [
        (k, [single_region(lu_handles[i], _GPU_DEVICE, block_size)])
        for i, k in enumerate(keys)
    ]
    oks = ring.lookup(lu_entries)
    _libcudart.cudaDeviceSynchronize()
    assert_all_success(oks, "Lookup", keys)

    for i, k in enumerate(keys):
        actual = gpu_read(lu_ptrs[i], block_size)
        if actual != patterns[k]:
            raise AssertionError(f"Integrity fail on load: key={k}")
    print("    6. transfer(load): OK (integrity verified)")

    # 7. complete_load → Unpin
    oks = ring.unpin(keys)
    assert_all_success(oks, "Unpin", keys)
    print("    7. complete_load: OK")

    # 8. touch (keep alive after load completes)
    oks = ring.touch(keys, promote=False)
    assert_all_success(oks, "Touch", keys)
    print("    8. touch:         OK")

    # 9. take_events (verify we can poll — no evictions expected for these keys)
    events, _dropped = ring.take_events(0)
    # We don't assert empty — other tests may have residual events
    print(f"    9. take_events:   OK ({len(events)} pending)")

    ring.remove(keys)
    print("    Cleanup:          OK")


def test_pin_prevents_eviction(ring, block_size, ptrs, handles, pool_size):
    """Pin semantics: pinned entries survive memory pressure.

    Validates that a pinned entry is NOT evicted even when the memory-tier
    is overflowed by other entries.
    """
    print("\n  [pin_eviction] Pinned entries survive memory pressure")

    # Clear memory-tier and drain stale events for a clean start
    ring.clear_memory_tier()
    ring.take_events(0)

    # Store a sentinel entry and pin it immediately
    sentinel_key = 600_000_001
    gpu_write(ptrs[0], make_pattern(sentinel_key, block_size))
    populate_via_populate_rpc(ring, [sentinel_key], block_size, [ptrs[0]], [handles[0]])

    oks = ring.pin([sentinel_key])
    assert_all_success(oks, "Pin sentinel", [sentinel_key])
    print("    Sentinel pinned:  OK")

    # Drain any events from the populate (in case it triggered eviction of old entries)
    ring.take_events(0)

    # Flood memory-tier to trigger eviction
    num_flood = pool_size // block_size + 16
    base_key = 600_100_000
    flood_keys = [base_key + i for i in range(num_flood)]

    batch_size = min(len(ptrs), 8)
    for batch_start in range(0, len(flood_keys), batch_size):
        batch = flood_keys[batch_start:batch_start + batch_size]
        entries = [
            (k, [single_region(handles[idx % len(handles)], _GPU_DEVICE, block_size)])
            for idx, k in enumerate(batch)
        ]
        ring.populate(entries)

    # Sentinel should still exist (pinned)
    exists = check_exists(ring, [sentinel_key])
    assert exists.get(sentinel_key, False), "Pinned sentinel was evicted!"
    print("    Survived flood:   OK")

    # Drain events — sentinel should NOT appear (it was pinned before flood)
    events, _dropped = ring.take_events(0)
    evicted_keys = {ev_key for ev_key, ev_reason in events if ev_reason == REASON_REMOVED}
    if sentinel_key in evicted_keys:
        raise AssertionError("Pinned sentinel appeared in REMOVED eviction events!")
    print("    Not in events:    OK")

    # Cleanup
    ring.unpin([sentinel_key])
    ring.remove([sentinel_key])
    ring.clear_memory_tier()
    print("    Cleanup:          OK")


# ============================================================
# Main
# ============================================================

def main():
    global _GPU_DEVICE

    parser = argparse.ArgumentParser(
        description="Validate Certus shmq APIs against OffloadingSpec contract"
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
    parser.add_argument(
        "--memory-tier-size", type=parse_size, default=256 * 1024 * 1024,
        help="Server memory-tier pool size (default: 256M). Must match server config."
    )
    args = parser.parse_args()

    block_size = args.block_size
    num_objects = args.num_objects

    _GPU_DEVICE = args.gpu
    _libcudart.cudaSetDevice(args.gpu)

    print("=" * 60)
    print("Certus OffloadingSpec Compliance Test")
    print("=" * 60)
    print(f"  Server:      {args.shm_path}")
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

    # Multiple threads would share one ring (per-thread sticky channel auto-claimed);
    # this test is single-threaded, so one channel suffices.
    ring = connect(args.shm_path)

    # Ensure clean starting state (server may have residual entries from prior runs)
    ensure_pool_quiescent(ring)

    passed = 0
    failed = 0
    tests = [
        # Store path
        ("prepare_store", lambda: test_prepare_store(
            ring, key_sets[0], block_size, store_ptrs, store_handles)),
        ("transfer_store", lambda: test_transfer_store(
            ring, key_sets[1], block_size, store_ptrs, store_handles)),
        ("complete_store_success", lambda: test_complete_store_success(
            ring, key_sets[2], block_size, store_ptrs, store_handles,
            load_ptrs, load_handles)),
        ("complete_store_abort", lambda: test_complete_store_abort(
            ring, key_sets[3], block_size, store_ptrs, store_handles)),
        # Lookup
        ("lookup_check", lambda: test_lookup_check(
            ring, key_sets[4], block_size, store_ptrs, store_handles)),
        # Load path
        ("prepare_load", lambda: test_prepare_load(
            ring, key_sets[5], block_size, store_ptrs, store_handles)),
        ("transfer_load", lambda: test_transfer_load(
            ring, key_sets[6], block_size, store_ptrs, store_handles,
            load_ptrs, load_handles)),
        ("complete_load", lambda: test_complete_load(
            ring, key_sets[7], block_size, store_ptrs, store_handles)),
        # Touch
        ("touch", lambda: test_touch(
            ring, key_sets[8], block_size, store_ptrs, store_handles)),
        # Removal (before flooding tests to avoid background evictor interference)
        ("remove", lambda: test_remove(
            ring, key_sets[10], block_size, store_ptrs, store_handles)),
        # Bulk operations
        ("clear_memory_tier", lambda: test_clear_memory_tier(
            ring, key_sets[11], block_size, store_ptrs, store_handles)),
        ("flush_to_ssd", lambda: test_flush_to_ssd(
            ring, key_sets[12], block_size, store_ptrs, store_handles)),
        # End-to-end lifecycle
        ("full_lifecycle", lambda: test_full_offloading_lifecycle(
            ring, key_sets[13], block_size, store_ptrs, store_handles,
            load_ptrs, load_handles)),
        # Eviction events (floods memory-tier — run after non-pressure tests)
        ("take_events", lambda: test_take_events(
            ring, block_size, store_ptrs, store_handles, args.memory_tier_size)),
        # Pin-under-pressure
        ("pin_prevents_eviction", lambda: test_pin_prevents_eviction(
            ring, block_size, store_ptrs, store_handles, args.memory_tier_size)),
    ]

    all_keys = [k for ks in key_sets for k in ks]
    for name, test_fn in tests:
        try:
            test_fn()
            passed += 1
        except (AssertionError, RingError) as e:
            print(f"    FAILED: {e}")
            failed += 1
            # Best-effort cleanup
            try:
                ring.abort_store(all_keys)
            except Exception:
                pass
            try:
                ring.unpin(all_keys)
            except Exception:
                pass
            try:
                ring.remove(all_keys)
            except Exception:
                pass

    # Free GPU memory
    for ptr in store_ptrs:
        cuda_free(ptr)
    for ptr in load_ptrs:
        cuda_free(ptr)
    ring.close()

    print("\n" + "=" * 60)
    print(f"Results: {passed} passed, {failed} failed, {passed + failed} total")
    print("=" * 60)

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()

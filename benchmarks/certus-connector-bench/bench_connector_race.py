#!/usr/bin/env python3
"""Reproduce the check→pin→lookup eviction race.

Simulates the vLLM connector workload that triggers the crash:
- Thread 1 (storer): continuously stores new blocks via 3-phase protocol,
  filling the memory tier and forcing eviction of old entries.
- Thread 2 (loader): loads COMMITTED keys (from a shared queue), doing
  check → pin → lookup → unpin, the connector's load sequence. Only loads keys
  the storer has fully committed, mimicking vLLM where the scheduler only loads
  blocks already stored.

The race: between the Check (which saw the key as resident) and the Pin, the
evictor can demote/remove the key. Pin succeeds only for a key that is present
and protects it from eviction until Unpin, so a Pin failure is the race being
caught. The loader should see ZERO LOAD failures for successfully-pinned keys.

Usage (server must be running):
    python bench_connector_race.py --shm-path /dev/shm/certus-shmq --duration 30
"""

import argparse
import collections
import ctypes
import os
import sys
import threading
import time

_here = os.path.dirname(os.path.abspath(__file__))
_repo = os.path.join(_here, '..', '..')
sys.path.insert(0, _here)
sys.path.insert(0, os.path.join(_repo, 'apps', 'python'))
sys.path.insert(0, os.path.join(_repo, 'certus-shmq-connector'))

import torch
from certus_shmq_helpers import single_region
from certus_shmq_connector.ring import CHECK_RESIDENT, Ring

assert torch.cuda.is_available(), "CUDA GPU required"

OBJECT_SIZE = 2 * 1024 * 1024  # 2 MiB (Llama-3-8B: 16 tokens × 32 layers × 4096 bytes)


def _get_cuda_ipc_handle(ptr):
    handle = (ctypes.c_byte * 64)()
    err = ctypes.CDLL("libcudart.so").cudaIpcGetMemHandle(
        ctypes.byref(handle), ctypes.c_void_p(ptr)
    )
    assert err == 0, f"cudaIpcGetMemHandle failed: {err}"
    return bytes(handle)


class Stats:
    def __init__(self):
        self.lock = threading.Lock()
        self.stores = 0
        self.store_errors = 0
        self.loads = 0
        self.load_ok = 0
        self.pin_failures = 0
        self.load_failures = 0
        self.check_misses = 0


def store_worker(ring, region, stats, batch_size, stop_event, committed_queue):
    """Continuously store new keys, pushing committed ranges to the queue."""
    key_base = 1_000_000
    batch_num = 0

    while not stop_event.is_set():
        keys = list(range(key_base + batch_num * batch_size,
                          key_base + (batch_num + 1) * batch_size))
        try:
            # Phase 1: Reserve
            reserve_reqs = [(k, OBJECT_SIZE, 0) for k in keys]
            reserved = ring.reserve(reserve_reqs)
            ok_keys = [k for k, ok in zip(keys, reserved) if ok]
            if not ok_keys:
                with stats.lock:
                    stats.store_errors += 1
                batch_num += 1
                continue

            # Phase 2: Copy
            entries = [(k, [region]) for k in ok_keys]
            copy_oks = ring.copy_to_store(entries)
            copied = [k for k, ok in zip(ok_keys, copy_oks) if ok]

            # Phase 3: Commit
            if copied:
                ring.commit_store(copied)
                # Signal to loader: these keys are now committed and loadable
                committed_queue.append(list(copied))

            # Abort any that reserved but didn't copy
            failed_keys = [k for k in ok_keys if k not in set(copied)]
            if failed_keys:
                try:
                    ring.abort_store(failed_keys)
                except Exception:
                    pass

            with stats.lock:
                stats.stores += len(copied)
                if len(copied) < len(keys):
                    stats.store_errors += 1

        except Exception:
            with stats.lock:
                stats.store_errors += 1

        batch_num += 1


def load_worker(ring, region, stats, batch_size, stop_event, committed_queue):
    """Load only keys that the store worker has committed."""
    # Wait for some stores to accumulate
    time.sleep(1.0)

    while not stop_event.is_set():
        # Get a batch of committed keys to load
        if not committed_queue:
            time.sleep(0.01)
            continue

        try:
            keys = committed_queue.popleft()
        except IndexError:
            time.sleep(0.01)
            continue

        try:
            # Check: which committed keys are still resident.
            states = ring.check_states(keys)
            resident = [k for k, s in zip(keys, states) if s == CHECK_RESIDENT]
            with stats.lock:
                stats.check_misses += len(keys) - len(resident)
            if not resident:
                continue

            # Pin: succeeds only for a key still present, and protects it from
            # eviction until Unpin. A failure is a key evicted after the Check.
            pin_oks = ring.pin(resident)
            pinned = []
            for k, ok in zip(resident, pin_oks):
                if ok:
                    pinned.append(k)
                else:
                    with stats.lock:
                        stats.pin_failures += 1

            if not pinned:
                continue

            # Lookup: should ALWAYS succeed for pinned keys (read_ref > 0
            # protects from eviction)
            entries = [(k, [region]) for k in pinned]
            try:
                lookup_oks = ring.lookup(entries)
            finally:
                # Unpin whatever happens to the lookup: a pin left behind
                # keeps its entry from ever being evicted.
                try:
                    ring.unpin(pinned)
                except Exception:
                    pass

            with stats.lock:
                stats.loads += len(pinned)
                for k, ok in zip(pinned, lookup_oks):
                    if ok:
                        stats.load_ok += 1
                    else:
                        stats.load_failures += 1
                        print(
                            f"  LOAD FAILURE key={k} (pinned, "
                            f"read_ref>0, should be impossible!)",
                            flush=True,
                        )

        except Exception:
            pass


def main():
    parser = argparse.ArgumentParser(description="Reproduce check→pin→lookup eviction race")
    parser.add_argument("--shm-path", default="/dev/shm/certus-shmq")
    parser.add_argument("--duration", type=float, default=30.0, help="Run duration in seconds")
    parser.add_argument("--bs", type=int, default=16, help="Batch size")
    parser.add_argument("--gpu", type=int, default=0)
    args = parser.parse_args()

    torch.cuda.set_device(args.gpu)

    # Allocate GPU buffers
    store_tensor = torch.randint(0, 256, (OBJECT_SIZE,), dtype=torch.uint8, device=f'cuda:{args.gpu}')
    store_handle = _get_cuda_ipc_handle(store_tensor.data_ptr())
    store_region = single_region(store_handle, args.gpu, OBJECT_SIZE)

    load_tensor = torch.empty(OBJECT_SIZE, dtype=torch.uint8, device=f'cuda:{args.gpu}')
    load_handle = _get_cuda_ipc_handle(load_tensor.data_ptr())
    load_region = single_region(load_handle, args.gpu, OBJECT_SIZE)

    # Two Ring connections on separate channels
    store_ring = Ring(args.shm_path, ready_timeout=10.0, log=lambda m: None,
                      claim_slot=0, claim_slots=2)
    load_ring = Ring(args.shm_path, ready_timeout=10.0, log=lambda m: None,
                     claim_slot=1, claim_slots=2)

    stats = Stats()
    stop_event = threading.Event()
    committed_queue = collections.deque()

    print(f"Running eviction race test for {args.duration}s (bs={args.bs})")
    print(f"Loader only loads COMMITTED keys (no concurrent-store overlap)")
    print()

    t_store = threading.Thread(target=store_worker,
                               args=(store_ring, store_region, stats, args.bs,
                                     stop_event, committed_queue))
    t_load = threading.Thread(target=load_worker,
                              args=(load_ring, load_region, stats, args.bs,
                                    stop_event, committed_queue))

    t_store.start()
    t_load.start()

    time.sleep(args.duration)
    stop_event.set()

    t_store.join(timeout=10)
    t_load.join(timeout=10)

    store_ring.close()
    load_ring.close()

    print()
    print("=" * 60)
    print("RESULTS")
    print("=" * 60)
    print(f"  Stores:        {stats.stores} ({stats.store_errors} errors)")
    print(f"  Loads:         {stats.loads}")
    print(f"  Load OK:       {stats.load_ok}")
    print(f"  PIN failures:  {stats.pin_failures}  (key evicted between Check and Pin)")
    print(f"  LOAD failures: {stats.load_failures}  (pinned key failed at lookup — should be 0!)")
    print(f"  Check misses:  {stats.check_misses}  (key not resident at Check)")
    print()

    if stats.load_failures > 0:
        print(f"  *** BUG: {stats.load_failures} pinned keys failed lookup ***")
        print(f"  This should be impossible — read_ref > 0 prevents eviction.")
    elif stats.pin_failures > 0 and stats.load_failures == 0:
        print(f"  OK: {stats.pin_failures} keys evicted between Check and Pin")
        print(f"  (expected under memory pressure), but ZERO load failures on pinned keys.")
        print(f"  Pin protects pinned keys correctly.")
    else:
        print(f"  CLEAN: no pin failures, no load failures.")


if __name__ == "__main__":
    main()

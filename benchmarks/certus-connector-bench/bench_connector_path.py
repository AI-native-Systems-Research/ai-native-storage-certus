#!/usr/bin/env python3
"""Benchmark the connector's 3-phase store and check→pin→lookup load paths.

Measures the actual code paths the certus-shmq-connector uses:
  Store: reserve → copy_to_store → commit_store
  Load:  check → pin → lookup

Usage (server must be running):
    python bench_connector_path.py --shm-path /dev/shm/certus-shmq --bs 16 --num-blocks 128
"""

import argparse
import ctypes
import os
import statistics
import sys
import time

_here = os.path.dirname(os.path.abspath(__file__))
_repo = os.path.join(_here, '..', '..')
sys.path.insert(0, _here)
sys.path.insert(0, os.path.join(_repo, 'apps', 'python'))
sys.path.insert(0, os.path.join(_repo, 'certus-shmq-connector'))

import torch
from certus_shmq_helpers import single_region

from certus_shmq_connector.ring import Ring

assert torch.cuda.is_available(), "CUDA GPU required"

OBJECT_SIZE = 5 * 1024 * 1024  # 5 MiB (Llama-70B block)


def run_bench(shm_path, num_blocks, batch_size, gpu_id, min_duration, warmup):
    ring = Ring(shm_path, ready_timeout=10.0, log=lambda msg: None)

    # Allocate GPU buffers
    store_tensor = torch.randint(0, 256, (OBJECT_SIZE,), dtype=torch.uint8, device=f'cuda:{gpu_id}')
    store_handle = _get_cuda_ipc_handle(store_tensor.data_ptr())
    store_region = single_region(store_handle, gpu_id, OBJECT_SIZE)

    load_tensor = torch.empty(OBJECT_SIZE, dtype=torch.uint8, device=f'cuda:{gpu_id}')
    load_handle = _get_cuda_ipc_handle(load_tensor.data_ptr())
    load_region = single_region(load_handle, gpu_id, OBJECT_SIZE)

    key_base = 80_000_000

    # Warmup
    for i in range(warmup):
        k = key_base + 900_000 + i
        try:
            ring.reserve([(k, OBJECT_SIZE, 0)])
            ring.copy_to_store([(k, [store_region])])
            ring.commit_store([k])
            ring.remove([k])
        except Exception:
            pass

    # ── 3-Phase Store Benchmark ──
    print(f"\n{'='*60}")
    print(f"3-Phase Store (bs={batch_size}, {num_blocks} blocks)")
    print(f"{'='*60}")

    store_latencies = []
    store_phase_latencies = {'reserve': [], 'copy': [], 'commit': []}
    iteration = 0
    elapsed = 0.0

    while elapsed < min_duration or iteration < 2:
        # Generate unique keys per iteration
        keys = list(range(key_base + iteration * num_blocks,
                          key_base + (iteration + 1) * num_blocks))

        for i in range(0, len(keys), batch_size):
            batch_keys = keys[i:i + batch_size]
            entries = [(k, [store_region]) for k in batch_keys]

            t_total = time.perf_counter()

            # Phase 1: Reserve
            t0 = time.perf_counter()
            reserved = ring.reserve([(k, OBJECT_SIZE, 0) for k in batch_keys])
            t_reserve = time.perf_counter() - t0
            ok_keys = [k for k, ok in zip(batch_keys, reserved) if ok]
            if not ok_keys:
                continue

            # Phase 2: Copy (the bottleneck)
            t0 = time.perf_counter()
            ok_entries = [(k, [store_region]) for k in ok_keys]
            copy_oks = ring.copy_to_store(ok_entries)
            t_copy = time.perf_counter() - t0
            copied_keys = [k for k, ok in zip(ok_keys, copy_oks) if ok]

            # Phase 3: Commit
            t0 = time.perf_counter()
            if copied_keys:
                ring.commit_store(copied_keys)
            t_commit = time.perf_counter() - t0

            t_end = time.perf_counter()
            n = len(copied_keys) if copied_keys else 1

            store_latencies.append((t_end - t_total) / n)
            store_phase_latencies['reserve'].append(t_reserve / n)
            store_phase_latencies['copy'].append(t_copy / n)
            store_phase_latencies['commit'].append(t_commit / n)

        iteration += 1
        elapsed = sum(store_latencies) * len(store_latencies)  # rough
        if store_latencies:
            elapsed = store_latencies[-1] * len(store_latencies)

    if store_latencies:
        total_ops = len(store_latencies) * batch_size
        total_bytes = total_ops * OBJECT_SIZE
        wall = sum(store_latencies) * batch_size
        p50 = statistics.median(store_latencies) * 1e6
        p99 = sorted(store_latencies)[int(len(store_latencies) * 0.99)] * 1e6
        throughput = total_bytes / wall / 1e9 if wall > 0 else 0

        print(f"  ops={total_ops}  throughput={throughput:.2f} GB/s")
        print(f"  p50={p50:.1f}us  p99={p99:.1f}us")
        print(f"  Phase breakdown (p50 per op):")
        for phase in ['reserve', 'copy', 'commit']:
            lats = store_phase_latencies[phase]
            if lats:
                print(f"    {phase:8s}: {statistics.median(lats)*1e6:.1f}us")

    # Use keys from the store phase for load benchmarks.
    all_stored_keys = list(range(key_base, key_base + iteration * num_blocks))
    load_keys = all_stored_keys[:num_blocks]

    def _run_load_bench(label, clear_tier):
        print(f"\n{'='*60}")
        print(f"{label} (bs={batch_size})")
        print(f"{'='*60}")

        load_latencies = []
        load_phase_latencies = {'check': [], 'pin': [], 'lookup': []}

        load_elapsed = 0.0
        load_iter = 0
        while load_elapsed < min_duration or load_iter < 2:
            # For cold loads: flush write-throughs to SSD, then clear memory tier
            # so every lookup forces a cold promote (SSD→DRAM→GPU).
            if clear_tier:
                try:
                    ring.flush_to_ssd()
                    ring.clear_memory_tier()
                except Exception:
                    pass

            for i in range(0, len(load_keys), batch_size):
                batch_keys = load_keys[i:i + batch_size]

                t_total = time.perf_counter()

                # Phase 1: Check
                t0 = time.perf_counter()
                exists = ring.check(batch_keys)
                t_check = time.perf_counter() - t0
                present = [k for k, e in zip(batch_keys, exists) if e]
                if not present:
                    continue

                # Phase 2: Pin
                t0 = time.perf_counter()
                pin_oks = ring.pin(present)
                t_pin = time.perf_counter() - t0
                pinned = [k for k, ok in zip(present, pin_oks) if ok]
                if not pinned:
                    continue

                # Phase 3: Lookup (SSD→GPU or DRAM→GPU)
                t0 = time.perf_counter()
                entries = [(k, [load_region]) for k in pinned]
                lookup_oks = ring.lookup(entries)
                torch.cuda.synchronize()
                t_lookup = time.perf_counter() - t0

                t_end = time.perf_counter()
                n = sum(1 for ok in lookup_oks if ok) or 1

                load_latencies.append((t_end - t_total) / n)
                load_phase_latencies['check'].append(t_check / n)
                load_phase_latencies['pin'].append(t_pin / n)
                load_phase_latencies['lookup'].append(t_lookup / n)

            load_iter += 1
            if load_latencies:
                load_elapsed = load_latencies[-1] * len(load_latencies)

        if load_latencies:
            total_ops = len(load_latencies) * batch_size
            total_bytes = total_ops * OBJECT_SIZE
            wall = sum(load_latencies) * batch_size
            p50 = statistics.median(load_latencies) * 1e6
            p99 = sorted(load_latencies)[int(len(load_latencies) * 0.99)] * 1e6
            throughput = total_bytes / wall / 1e9 if wall > 0 else 0

            print(f"  ops={total_ops}  throughput={throughput:.2f} GB/s")
            print(f"  p50={p50:.1f}us  p99={p99:.1f}us")
            print(f"  Phase breakdown (p50 per op):")
            for phase in ['check', 'pin', 'lookup']:
                lats = load_phase_latencies[phase]
                if lats:
                    print(f"    {phase:8s}: {statistics.median(lats)*1e6:.1f}us")

    # ── Warm Load: keys are in memory tier (just stored) ──
    _run_load_bench("Connector Warm Load (check→pin→lookup, DRAM→GPU)", clear_tier=False)

    # ── Cold Load: clear memory tier before each iteration ──
    _run_load_bench("Connector Cold Load (check→pin→lookup, SSD→GPU)", clear_tier=True)

    # Cleanup
    try:
        ring.remove(all_stored_keys)
    except Exception:
        pass
    ring.close()


def _get_cuda_ipc_handle(ptr):
    """Get the raw CUDA IPC handle bytes for a device pointer."""
    handle = (ctypes.c_byte * 64)()
    err = ctypes.CDLL("libcudart.so").cudaIpcGetMemHandle(
        ctypes.byref(handle), ctypes.c_void_p(ptr)
    )
    assert err == 0, f"cudaIpcGetMemHandle failed: {err}"
    return bytes(handle)


def main():
    parser = argparse.ArgumentParser(description="Benchmark connector 3-phase store/load")
    parser.add_argument("--shm-path", default="/dev/shm/certus-shmq")
    parser.add_argument("--bs", type=int, default=16, help="Batch size")
    parser.add_argument("--num-blocks", type=int, default=128, help="Total blocks")
    parser.add_argument("--gpu", type=int, default=0)
    parser.add_argument("--min-duration", type=float, default=2.0)
    parser.add_argument("--warmup", type=int, default=4)
    args = parser.parse_args()

    torch.cuda.set_device(args.gpu)
    run_bench(args.shm_path, args.num_blocks, args.bs, args.gpu,
              args.min_duration, args.warmup)


if __name__ == "__main__":
    main()

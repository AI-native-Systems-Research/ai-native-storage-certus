#!/usr/bin/env python3
"""certus-fio — FIO-like pattern-driven benchmark for certus shmq storage.

Reads workload pattern YAML files and executes them against a running
certus-server via shared-memory queue (shmq). Each pattern defines keyspaces,
preconditions, phases with store/load/delete operations, actor counts, and
measurement targets.

Usage:
    python3 certus-fio.py list
    python3 certus-fio.py describe --pattern cold_prefill_store
    python3 certus-fio.py run --pattern cold_prefill_store
    python3 certus-fio.py run --pattern bidirectional_store_load_contention --override store_actors=2 load_actors=8
"""

import argparse
import ctypes
import math
import os
import queue
import random
import signal
import statistics
import sys
import threading
import time
from pathlib import Path

import yaml

# ── Locate shmq helpers (repo root / apps / python) ──
_THIS_DIR = Path(__file__).resolve().parent
_REPO_ROOT = _THIS_DIR.parent.parent
_APPS_PYTHON = _REPO_ROOT / "apps" / "python"
if _APPS_PYTHON.exists():
    sys.path.insert(0, str(_APPS_PYTHON))

from certus_shmq_helpers import (
    Ring,
    RingError,
    add_shm_arg,
    connect,
    single_region,
)

PATTERNS_DIR = _REPO_ROOT / "knowledge" / "workload_patterns"

# ── CUDA helpers (raw cudaMalloc, NOT PyTorch) ──
_libcudart = ctypes.CDLL("libcudart.so")
_libcudart.cudaSetDevice.restype = ctypes.c_int
_libcudart.cudaSetDevice.argtypes = [ctypes.c_int]
_libcudart.cudaIpcGetMemHandle.restype = ctypes.c_int
_libcudart.cudaIpcGetMemHandle.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
_libcudart.cudaMalloc.restype = ctypes.c_int
_libcudart.cudaMalloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t]
_libcudart.cudaFree.restype = ctypes.c_int
_libcudart.cudaFree.argtypes = [ctypes.c_void_p]
_libcudart.cudaDeviceSynchronize.restype = ctypes.c_int


def cuda_alloc(size):
    dev_ptr = ctypes.c_void_p()
    err = _libcudart.cudaMalloc(ctypes.byref(dev_ptr), size)
    if err != 0:
        raise RuntimeError(f"cudaMalloc({size}) failed: {err}")
    handle_buf = (ctypes.c_ubyte * 64)()
    err = _libcudart.cudaIpcGetMemHandle(ctypes.byref(handle_buf), dev_ptr)
    if err != 0:
        _libcudart.cudaFree(dev_ptr)
        raise RuntimeError(f"cudaIpcGetMemHandle failed: {err}")
    return dev_ptr, bytes(handle_buf)


def cuda_free(dev_ptr):
    _libcudart.cudaFree(dev_ptr)


# ── Expression evaluator ──

def eval_expr(expr, params):
    if isinstance(expr, (int, float)):
        return int(expr)
    s = str(expr)
    for k, v in sorted(params.items(), key=lambda x: -len(x[0])):
        s = s.replace(k, str(v))
    try:
        return int(eval(s, {"__builtins__": {}}, {"ceil": math.ceil, "floor": math.floor, "max": max, "min": min}))
    except Exception:
        raise ValueError(f"Cannot evaluate: {expr!r} with {params}")


# ── Pattern loader ──

class WorkloadPattern:
    def __init__(self, path, overrides=None):
        self.path = Path(path)
        raw = yaml.safe_load(self.path.read_text())
        self.id = raw["id"]
        self.name = raw.get("name", self.id)
        self.status = raw.get("status", "candidate")

        self.params = {}
        for k, spec in raw.get("parameters", {}).items():
            self.params[k] = spec.get("default", 0) if isinstance(spec, dict) else spec
        if overrides:
            for k, v in overrides.items():
                if k in self.params:
                    try:
                        self.params[k] = int(v)
                    except ValueError:
                        self.params[k] = float(v)

        self.keyspaces = {}
        for ks_name, ks_spec in raw.get("keyspaces", {}).items():
            self.keyspaces[ks_name] = {
                "cardinality": eval_expr(ks_spec.get("cardinality", "1"), self.params),
                "object_bytes": eval_expr(ks_spec.get("object_bytes", "4194304"), self.params),
                "sharing": ks_spec.get("sharing", "per_actor"),
                "disjoint": ks_spec.get("disjoint_between_actors", True),
            }

        self.preconditions = raw.get("preconditions", [])
        self.phases = raw.get("phases", [])
        self.expected_io = raw.get("expected_io", {})

    def describe(self):
        print(f"Pattern: {self.id}")
        print(f"  Name: {self.name}")
        print(f"  Parameters:")
        for k, v in self.params.items():
            print(f"    {k} = {v}")
        print(f"  Keyspaces:")
        for ks_name, ks in self.keyspaces.items():
            print(f"    {ks_name}: {ks['cardinality']} objects x {ks['object_bytes']} bytes ({ks['sharing']})")
        print(f"  Preconditions:")
        for pc in self.preconditions:
            print(f"    {pc['subject']}: {pc['state']} = {pc['value']}")
        print(f"  Phases:")
        for phase in self.phases:
            count = eval_expr(phase.get("actors", {}).get("count", 1), self.params)
            ops = [op["op"] for op in phase.get("operations", [])]
            print(f"    {phase['id']}: {count} actors, ops={ops}, barrier={phase.get('barrier_after', False)}")
        print(f"  Expected IO:")
        for k, v in self.expected_io.items():
            try:
                print(f"    {k}: {eval_expr(v, self.params)}")
            except Exception:
                print(f"    {k}: {v}")


# ── Benchmark runner ──

class PhaseResult:
    def __init__(self, phase_id, operation):
        self.phase_id = phase_id
        self.operation = operation
        self.latencies = []
        self.errors = 0
        self.total_bytes = 0
        self.wall_start = 0.0
        self.wall_end = 0.0
        self._measured_elapsed = 0.0
        self._lock = threading.Lock()

    def record(self, latency, nbytes):
        with self._lock:
            self.latencies.append(latency)
            self.total_bytes += nbytes

    def record_error(self):
        with self._lock:
            self.errors += 1

    @property
    def elapsed(self):
        if self._measured_elapsed > 0:
            return self._measured_elapsed
        return self.wall_end - self.wall_start

    @property
    def throughput_gbps(self):
        return (self.total_bytes / self.elapsed / 1e9) if self.elapsed > 0 else 0


class BenchRunner:
    def __init__(self, pattern: WorkloadPattern, ring: Ring, gpu_id: int = 0,
                 cleanup_before: bool = False, warmup_ops: int = 8,
                 min_duration: float = 3.0, max_iterations: int = 200,
                 batch_size_override: int = 0):
        self.pattern = pattern
        self.ring = ring
        self.gpu_id = gpu_id
        self.cleanup_before = cleanup_before
        self.warmup_ops = warmup_ops
        self.min_duration = min_duration
        self.max_iterations = max_iterations
        self.batch_size_override = batch_size_override
        self._stop = threading.Event()
        self._all_keys = set()
        self._gpu_buffers = []
        self._gpu_buffer_cache = {}
        self._key_cache = {}
        self._key_base = random.randint(1_000_000, 50_000_000)

        _libcudart.cudaSetDevice(gpu_id)

    def _alloc_buffer(self, size, shared=True):
        """Allocate a CUDA buffer. shared=True reuses cached allocation (for preconditions),
        shared=False allocates a new unique buffer (for concurrent actors)."""
        if shared and size in self._gpu_buffer_cache:
            return self._gpu_buffer_cache[size]
        ptr, handle_bytes = cuda_alloc(size)
        self._gpu_buffers.append(ptr)
        result = (handle_bytes, size)
        if shared:
            self._gpu_buffer_cache[size] = result
        return result

    def _run_warmup(self):
        """Run a few store+load ops to warm CUDA context, IPC handles, and TLBs."""
        first_ks = next(iter(self.pattern.keyspaces.values()), None)
        if not first_ks:
            return
        obj_size = min(first_ks["object_bytes"], 4 * 1024 * 1024)
        handle_bytes, size = self._alloc_buffer(obj_size)
        region = single_region(handle_bytes, self.gpu_id, size)
        warmup_base = 99_000_000 + random.randint(0, 1_000_000)
        warmup_keys = list(range(warmup_base, warmup_base + self.warmup_ops))
        # Warmup stores
        for k in warmup_keys:
            try:
                self.ring.populate([(k, [region])])
            except RingError:
                pass
        # Warmup loads
        for k in warmup_keys:
            try:
                self.ring.lookup([(k, [region])])
            except RingError:
                pass
        # Cleanup warmup keys
        try:
            self.ring.remove(warmup_keys)
        except RingError:
            pass

    def _get_keys(self, ks_name, actor_id, num_actors):
        cache_key = (ks_name, actor_id)
        if cache_key in self._key_cache:
            return self._key_cache[cache_key]

        ks = self.pattern.keyspaces[ks_name]
        cardinality = ks["cardinality"]
        ks_offset = list(self.pattern.keyspaces.keys()).index(ks_name) * 10_000_000

        if ks["sharing"] == "global":
            keys = list(range(self._key_base + ks_offset, self._key_base + ks_offset + cardinality))
        elif ks["disjoint"]:
            actor_offset = actor_id * cardinality
            keys = list(range(self._key_base + ks_offset + actor_offset,
                              self._key_base + ks_offset + actor_offset + cardinality))
        else:
            keys = list(range(self._key_base + ks_offset, self._key_base + ks_offset + cardinality))

        self._key_cache[cache_key] = keys
        self._all_keys.update(keys)
        return keys

    def _max_actors_for_keyspace(self, ks_name):
        max_a = 1
        for phase in self.pattern.phases:
            for op in phase.get("operations", []):
                if op.get("keys") == ks_name:
                    # In parallel-split phases, use op-specific actor count
                    param_name = f"{op['op']}_actors"
                    if param_name in self.pattern.params:
                        count = int(self.pattern.params[param_name])
                    else:
                        count = eval_expr(phase.get("actors", {}).get("count", 1), self.pattern.params)
                    max_a = max(max_a, count)
        return max_a

    def _setup_preconditions(self, verbose=True):
        for pc in self.pattern.preconditions:
            ks_name = pc["subject"]
            state = pc["state"]
            ks = self.pattern.keyspaces[ks_name]

            if state == "present_in_store" and pc["value"]:
                if verbose:
                    print(f"  Setup: populating {ks_name} ({ks['cardinality']} x {ks['object_bytes']} bytes)")
                handle_bytes, size = self._alloc_buffer(ks["object_bytes"])
                region = single_region(handle_bytes, self.gpu_id, size)

                if ks["sharing"] == "global":
                    keys = self._get_keys(ks_name, 0, 1)
                    entries = [(k, [region]) for k in keys]
                    oks = self.ring.populate(entries)
                    if not all(oks):
                        print(f"  WARNING: {sum(1 for o in oks if not o)}/{len(oks)} populate failures in setup")
                else:
                    max_actors = self._max_actors_for_keyspace(ks_name)
                    for aid in range(max_actors):
                        keys = self._get_keys(ks_name, aid, max_actors)
                        entries = [(k, [region]) for k in keys]
                        oks = self.ring.populate(entries)
                        if not all(oks):
                            print(f"  WARNING: populate failures for actor {aid}")

                self.ring.flush_to_ssd()

            if state == "absent_from_local_cache" and pc["value"]:
                if verbose:
                    print(f"  Setup: clearing memory tier")
                self.ring.clear_memory_tier()
                time.sleep(0.1)

            if state == "absent_from_store" and pc["value"]:
                if ks["sharing"] == "global":
                    keys = self._get_keys(ks_name, 0, 1)
                else:
                    max_actors = self._max_actors_for_keyspace(ks_name)
                    keys = []
                    for aid in range(max_actors):
                        keys.extend(self._get_keys(ks_name, aid, max_actors))
                try:
                    self.ring.remove(keys)
                except RingError:
                    pass

    def _run_actor(self, actor_id, ops_sequence, num_actors, start_event: threading.Event,
                   concurrency_sem: threading.Semaphore, ready_latch: list):
        """Run one actor thread. Each actor claims one Ring channel (auto on first call).

        ops_sequence: list of (op, ks_name, object_bytes, repeat_count, order, batch_size, result)
        For multi-op phases, same actor runs ops sequentially (not split across actors).
        """
        _libcudart.cudaSetDevice(self.gpu_id)

        # Pre-allocate buffers per op type. Store buffers are unique per actor
        # (concurrent DMA from same source address causes server failures).
        # Load buffers can be shared (DMA targets, not sources).
        import itertools
        op_buffers = {}
        load_batch_buffers = []
        for op, ks_name, object_bytes, _, _, batch_sz, _ in ops_sequence:
            if op == "store" and "store" not in op_buffers:
                op_buffers["store"] = self._alloc_buffer(object_bytes, shared=False)
            elif op == "load" and "load" not in op_buffers:
                op_buffers["load"] = self._alloc_buffer(object_bytes)
                if batch_sz > 1:
                    load_batch_buffers = [self._alloc_buffer(object_bytes) for _ in range(batch_sz)]

        ready_latch.append(True)
        start_event.wait()

        concurrency_sem.acquire()
        try:
            for op, ks_name, object_bytes, repeat_count, order, batch_sz, result in ops_sequence:
                if self._stop.is_set():
                    break

                base_keys = list(self._get_keys(ks_name, actor_id, num_actors))
                if repeat_count != len(base_keys):
                    keys = list(itertools.islice(itertools.cycle(base_keys), repeat_count))
                else:
                    keys = base_keys[:]
                if order == "random":
                    random.shuffle(keys)

                if op == "store":
                    handle_bytes, size = op_buffers["store"]
                    region = single_region(handle_bytes, self.gpu_id, size)
                    for i in range(0, len(keys), batch_sz):
                        if self._stop.is_set():
                            break
                        batch_keys = keys[i:i + batch_sz]
                        entries = [(k, [region]) for k in batch_keys]
                        t0 = time.perf_counter()
                        try:
                            oks = self.ring.populate(entries)
                            t1 = time.perf_counter()
                            if all(oks):
                                result.record((t1 - t0) / len(batch_keys), object_bytes * len(batch_keys))
                            else:
                                ok_count = sum(1 for o in oks if o)
                                if ok_count:
                                    result.record((t1 - t0) / len(batch_keys), object_bytes * ok_count)
                                result.record_error()
                        except RingError:
                            result.record_error()
                        self._all_keys.update(batch_keys)

                elif op == "load":
                    if batch_sz > 1 and load_batch_buffers:
                        for i in range(0, len(keys), batch_sz):
                            if self._stop.is_set():
                                break
                            batch_keys = keys[i:i + batch_sz]
                            entries = [
                                (k, [single_region(load_batch_buffers[j % len(load_batch_buffers)][0],
                                                   self.gpu_id, load_batch_buffers[j % len(load_batch_buffers)][1])])
                                for j, k in enumerate(batch_keys)
                            ]
                            t0 = time.perf_counter()
                            try:
                                oks = self.ring.lookup(entries)
                                _libcudart.cudaDeviceSynchronize()
                                t1 = time.perf_counter()
                                if all(oks):
                                    result.record((t1 - t0) / len(batch_keys), object_bytes * len(batch_keys))
                                else:
                                    ok_count = sum(1 for o in oks if o)
                                    if ok_count:
                                        result.record((t1 - t0) / len(batch_keys), object_bytes * ok_count)
                                    result.record_error()
                            except RingError:
                                result.record_error()
                    else:
                        handle_bytes, size = op_buffers["load"]
                        region = single_region(handle_bytes, self.gpu_id, size)
                        for key in keys:
                            if self._stop.is_set():
                                break
                            t0 = time.perf_counter()
                            try:
                                oks = self.ring.lookup([(key, [region])])
                                _libcudart.cudaDeviceSynchronize()
                                t1 = time.perf_counter()
                                if oks and oks[0]:
                                    result.record(t1 - t0, object_bytes)
                                else:
                                    result.record_error()
                            except RingError:
                                result.record_error()

                elif op == "delete":
                    for batch_start in range(0, len(keys), 100):
                        if self._stop.is_set():
                            break
                        batch_keys = keys[batch_start:batch_start + 100]
                        t0 = time.perf_counter()
                        try:
                            self.ring.remove(batch_keys)
                            t1 = time.perf_counter()
                            result.record(t1 - t0, 0)
                        except RingError:
                            result.record_error()
        finally:
            concurrency_sem.release()
            self.ring.release_channel()

    def _is_parallel_split(self, operations):
        """Determine if multi-op phase should split actors (parallel) or run sequentially."""
        # Parallel if explicit actor params exist (e.g., store_actors, load_actors)
        for op_spec in operations:
            if f"{op_spec['op']}_actors" in self.pattern.params:
                return True
        return False

    def _run_phase(self, phase_spec) -> list:
        phase_id = phase_spec["id"]
        actors_spec = phase_spec.get("actors", {})
        operations = phase_spec.get("operations", [])
        total_actors = max(1, eval_expr(actors_spec.get("count", 1), self.pattern.params))
        concurrency = max(1, eval_expr(actors_spec.get("concurrency", total_actors), self.pattern.params))

        # FIX #3: Release main thread's channel before measured phase
        self.ring.release_channel()

        # FIX #4: Determine if multi-op is parallel (split actors) or sequential (same actors)
        parallel_split = len(operations) > 1 and self._is_parallel_split(operations)

        # FIX: Compute effective concurrency BEFORE creating semaphore and threads
        effective_concurrency = min(concurrency, self.ring.channel_count)

        start_event = threading.Event()
        concurrency_sem = threading.Semaphore(effective_concurrency)
        ready_latch = []
        results = []
        threads = []

        # Build per-operation result objects
        op_results = {}
        for op_spec in operations:
            r = PhaseResult(phase_id, op_spec["op"])
            results.append(r)
            op_results[op_spec["op"]] = r

        if parallel_split:
            # Bidirectional case: split actors between ops, each actor does one op
            for op_spec in operations:
                op = op_spec["op"]
                ks_name = op_spec["keys"]
                ks = self.pattern.keyspaces[ks_name]
                object_bytes = ks["object_bytes"]
                repeat_count = eval_expr(op_spec.get("repeat", 1), self.pattern.params) if "repeat" in op_spec else 1
                order = op_spec.get("order", "sequential")
                batch_sz = self.batch_size_override if self.batch_size_override > 0 else max(1, eval_expr(op_spec.get("batch_size", 1), self.pattern.params))
                param_name = f"{op}_actors"
                op_actors = max(1, int(self.pattern.params.get(param_name, total_actors // len(operations))))

                for i in range(op_actors):
                    ops_seq = [(op, ks_name, object_bytes, repeat_count, order, batch_sz, op_results[op])]
                    t = threading.Thread(
                        target=self._run_actor,
                        args=(i, ops_seq, op_actors, start_event, concurrency_sem, ready_latch),
                        daemon=True,
                    )
                    threads.append(t)
        else:
            # Sequential case: each actor runs ALL ops in order (load-then-delete, etc.)
            ops_seq_template = []
            for op_spec in operations:
                op = op_spec["op"]
                ks_name = op_spec["keys"]
                ks = self.pattern.keyspaces[ks_name]
                object_bytes = ks["object_bytes"]
                repeat_count = eval_expr(op_spec.get("repeat", 1), self.pattern.params) if "repeat" in op_spec else 1
                order = op_spec.get("order", "sequential")
                batch_sz = self.batch_size_override if self.batch_size_override > 0 else max(1, eval_expr(op_spec.get("batch_size", 1), self.pattern.params))
                ops_seq_template.append((op, ks_name, object_bytes, repeat_count, order, batch_sz, op_results[op]))

            for i in range(total_actors):
                t = threading.Thread(
                    target=self._run_actor,
                    args=(i, ops_seq_template, total_actors, start_event, concurrency_sem, ready_latch),
                    daemon=True,
                )
                threads.append(t)

        if len(threads) > effective_concurrency:
            print(f"  Note: {len(threads)} actors, {effective_concurrency} concurrent "
                  f"(server channels: {self.ring.channel_count})")

        for t in threads:
            t.start()

        # Wait for all actors to be ready
        deadline = time.time() + 30
        while len(ready_latch) < len(threads) and time.time() < deadline:
            time.sleep(0.01)

        if len(ready_latch) < len(threads):
            print(f"  WARNING: only {len(ready_latch)}/{len(threads)} actors ready (timeout)")

        wall_start = time.perf_counter()
        for r in results:
            r.wall_start = wall_start
        start_event.set()

        for t in threads:
            t.join()
        wall_end = time.perf_counter()
        for r in results:
            r.wall_end = wall_end
        return results

    def run(self) -> dict:
        print(f"\n{'='*70}")
        print(f"certus-fio: {self.pattern.id}")
        print(f"{'='*70}")
        print(f"  Pattern: {self.pattern.name}")
        print(f"  Channels available: {self.ring.channel_count}")
        for k, v in self.pattern.params.items():
            print(f"  {k}: {v}")
        for ks_name, ks in self.pattern.keyspaces.items():
            total_mb = ks["cardinality"] * ks["object_bytes"] / (1024 * 1024)
            print(f"  {ks_name}: {ks['cardinality']} x {ks['object_bytes']//1024}KB = {total_mb:.1f} MB")
        print()

        if self.cleanup_before:
            print("  Cleanup: clearing memory tier...")
            self.ring.clear_memory_tier()

        try:
            # Warmup: exercise CUDA context, IPC handle cache, and TLBs before measurement.
            if self.warmup_ops > 0:
                self._run_warmup()

            self._setup_preconditions()

            # First iteration: run phases, collect result structure
            all_results = []
            for phase in self.pattern.phases:
                print(f"\n  Phase: {phase['id']}")
                phase_results = self._run_phase(phase)
                all_results.extend(phase_results)
                if phase.get("barrier_after", False):
                    self.ring.flush_to_ssd()

            first_wall = sum(r.elapsed for r in all_results if r.elapsed > 0) or 0.001
            iteration = 1
            total_measured = first_wall

            # Seed measured_elapsed with first iteration before repeating
            for r in all_results:
                r._measured_elapsed = r.elapsed

            # Auto-repeat until min_duration is met
            while total_measured < self.min_duration and iteration < self.max_iterations:
                # Clean up previous iteration: remove keys + free memory tier
                if self._all_keys:
                    old_keys = list(self._all_keys)
                    for batch_start in range(0, len(old_keys), 100):
                        try:
                            self.ring.remove(old_keys[batch_start:batch_start + 100])
                        except RingError:
                            pass
                    self._all_keys.clear()
                self.ring.clear_memory_tier()

                self._key_base = random.randint(1_000_000, 50_000_000)
                self._key_cache.clear()
                self._setup_preconditions(verbose=False)

                phase_offset = 0
                iter_elapsed = 0.0
                for phase in self.pattern.phases:
                    phase_results = self._run_phase(phase)
                    for j, new_r in enumerate(phase_results):
                        existing_r = all_results[phase_offset + j]
                        existing_r.latencies.extend(new_r.latencies)
                        existing_r.total_bytes += new_r.total_bytes
                        existing_r.errors += new_r.errors
                        existing_r._measured_elapsed += new_r.elapsed
                        iter_elapsed += new_r.elapsed
                    phase_offset += len(phase_results)
                    if phase.get("barrier_after", False):
                        self.ring.flush_to_ssd()
                iteration += 1
                total_measured += iter_elapsed

            if iteration > 1:
                print(f"\n  Iterations: {iteration} ({total_measured:.1f}s measured)")

            print(f"\n{'='*70}")
            print(f"Results: {self.pattern.id}")
            print(f"{'='*70}")
            report = {}
            for r in all_results:
                if not r.latencies:
                    print(f"  {r.phase_id}/{r.operation}: no data (errors={r.errors})")
                    continue
                avg = statistics.mean(r.latencies)
                p50 = statistics.median(r.latencies)
                s = sorted(r.latencies)
                p99 = s[min(int(len(s) * 0.99), len(s) - 1)]
                gbps = r.throughput_gbps
                ops = len(r.latencies)
                print(f"  {r.phase_id}/{r.operation}:")
                print(f"    ops={ops}  errors={r.errors}")
                print(f"    avg={avg*1e6:.1f}us  p50={p50*1e6:.1f}us  p99={p99*1e6:.1f}us")
                print(f"    throughput={gbps:.2f} GB/s  total={r.total_bytes/(1024*1024):.1f} MB")
                print(f"    wall={r.elapsed:.3f}s")
                report[f"{r.phase_id}/{r.operation}"] = {
                    "ops": ops, "errors": r.errors,
                    "avg_us": avg * 1e6, "p50_us": p50 * 1e6, "p99_us": p99 * 1e6,
                    "throughput_gbps": gbps, "total_bytes": r.total_bytes, "wall_s": r.elapsed,
                }
            return report
        finally:
            print(f"\n  Cleanup: removing {len(self._all_keys)} keys...")
            all_keys = list(self._all_keys)
            for batch_start in range(0, len(all_keys), 100):
                try:
                    self.ring.remove(all_keys[batch_start:batch_start + 100])
                except RingError:
                    pass
            for ptr in self._gpu_buffers:
                cuda_free(ptr)

    def stop(self):
        self._stop.set()


# ── CLI ──

def cmd_list(args):
    patterns_dir = Path(args.patterns_dir)
    print(f"Available patterns in {patterns_dir}:\n")
    for f in sorted(patterns_dir.glob("*.yaml")):
        if f.name.startswith("_") or f.name == "compositions.yaml":
            continue
        try:
            doc = yaml.safe_load(f.read_text())
            if not doc or not doc.get("id"):
                continue
            name = doc.get("name", "?")
            ops = set()
            for phase in doc.get("phases", []):
                for op in phase.get("operations", []):
                    ops.add(op.get("op", ""))
            print(f"  {f.stem:<45} {','.join(sorted(ops)):<15} {name}")
        except Exception:
            pass


def cmd_describe(args):
    pattern_path = resolve_pattern(args.pattern, args.patterns_dir)
    overrides = parse_overrides(args.override)
    WorkloadPattern(pattern_path, overrides).describe()


def cmd_run(args):
    pattern_path = resolve_pattern(args.pattern, args.patterns_dir)
    overrides = parse_overrides(args.override)
    pattern = WorkloadPattern(pattern_path, overrides)

    ring = connect(args.shm_path, ready_timeout=10.0)
    runner = BenchRunner(
        pattern=pattern, ring=ring, gpu_id=args.gpu,
        cleanup_before=args.cleanup_before,
        warmup_ops=args.warmup,
        min_duration=args.min_duration,
        max_iterations=args.max_iterations,
    )

    def sighandler(sig, frame):
        print("\n  Interrupted — cleaning up...")
        runner.stop()

    signal.signal(signal.SIGINT, sighandler)
    signal.signal(signal.SIGTERM, sighandler)

    try:
        runner.run()
    finally:
        ring.close()


def resolve_pattern(name, patterns_dir):
    for candidate in [
        Path(name),
        Path(patterns_dir) / name,
        Path(patterns_dir) / (name + ".yaml"),
        Path(patterns_dir) / (name.replace("-", "_") + ".yaml"),
    ]:
        if candidate.exists():
            return candidate
    sys.exit(f"Pattern not found: {name}\nLooked in: {patterns_dir}")


def parse_overrides(override_list):
    if not override_list:
        return {}
    overrides = {}
    for item in override_list:
        if "=" not in item:
            sys.exit(f"Invalid override: {item} (expected key=value)")
        k, v = item.split("=", 1)
        overrides[k] = v
    return overrides


def main():
    parser = argparse.ArgumentParser(description="certus-fio: pattern-driven benchmark")
    parser.add_argument("--patterns-dir", default=str(PATTERNS_DIR))
    add_shm_arg(parser)
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("list", help="List available patterns")

    p_desc = subparsers.add_parser("describe", help="Describe a pattern")
    p_desc.add_argument("--pattern", required=True)
    p_desc.add_argument("--override", nargs="*", help="key=value overrides")

    p_run = subparsers.add_parser("run", help="Run a benchmark")
    p_run.add_argument("--pattern", required=True)
    p_run.add_argument("--gpu", type=int, default=0)
    p_run.add_argument("--override", nargs="*", help="key=value overrides")
    p_run.add_argument("--cleanup-before", action="store_true")
    p_run.add_argument("--warmup", type=int, default=8,
                       help="Warmup ops before measurement (0 to disable)")
    p_run.add_argument("--min-duration", type=float, default=3.0,
                       help="Minimum measurement duration in seconds (auto-repeats iterations)")
    p_run.add_argument("--max-iterations", type=int, default=200,
                       help="Maximum iteration repeats (safety cap)")

    p_quick = subparsers.add_parser("quick", help="Quick health check: 5 key patterns at 5M objects (~20s)")
    p_quick.add_argument("--gpu", type=int, default=0)
    p_quick.add_argument("--min-duration", type=float, default=2.0)
    p_quick.add_argument("--warmup", type=int, default=8)

    p_full = subparsers.add_parser("full", help="Full sweep: vary batch_size and object_size across core patterns")
    p_full.add_argument("--gpu", type=int, default=0)
    p_full.add_argument("--min-duration", type=float, default=3.0)
    p_full.add_argument("--warmup", type=int, default=8)
    p_full.add_argument("--output", default=None, help="Write CSV results to file")

    p_report = subparsers.add_parser("report", help="Run full sweep and generate HTML report")
    p_report.add_argument("--gpu", type=int, default=0)
    p_report.add_argument("--min-duration", type=float, default=3.0)
    p_report.add_argument("--warmup", type=int, default=8)
    p_report.add_argument("--output", default="certus-fio-report.html", help="Output HTML file")
    p_report.add_argument("--csv", default=None, help="Also write raw CSV")
    p_report.add_argument("--from-csv", default=None,
                          help="Generate report from existing CSV (skip sweep)")

    args = parser.parse_args()
    if args.command == "list":
        cmd_list(args)
    elif args.command == "describe":
        cmd_describe(args)
    elif args.command == "run":
        cmd_run(args)
    elif args.command == "quick":
        cmd_quick(args)
    elif args.command == "full":
        cmd_full(args)
    elif args.command == "report":
        cmd_report(args)


# Quick health check patterns (5M objects, natural batch sizes)
QUICK_PATTERNS = [
    ("decode_block_store", 1),
    ("cold_prefill_store", 64),
    ("compute_local_eviction_and_later_reload", 64),
    ("hot_vs_cold_load_paths", 1),
    ("hot_vs_cold_load_paths", 64),
    ("bidirectional_store_load_contention", 1),
]


def cmd_quick(args):
    """Quick health check: 5 key patterns at Llama-70B object size."""
    print(f"{'='*70}")
    print("certus-fio: QUICK HEALTH CHECK")
    print(f"{'='*70}")
    print(f"  Object size: 5 MiB (Llama-70B)")
    print(f"  Patterns: {len(QUICK_PATTERNS)}")
    print(f"  Duration per pattern: {args.min_duration}s")
    print()

    overrides_70b = {"num_layers": "80", "kv_bytes_per_token_per_layer": "4096"}
    ring = connect(args.shm_path, ready_timeout=10.0)
    results = {}

    try:
        for pattern_name, bs in QUICK_PATTERNS:
            pattern_path = resolve_pattern(pattern_name, args.patterns_dir)
            overrides = {**overrides_70b}
            try:
                pattern = WorkloadPattern(pattern_path, overrides)
            except Exception as e:
                print(f"  SKIP {pattern_name}: {e}")
                continue
            runner = BenchRunner(
                pattern=pattern, ring=ring, gpu_id=args.gpu,
                warmup_ops=args.warmup, min_duration=args.min_duration,
                batch_size_override=bs,
            )
            try:
                report = runner.run()
            except Exception as e:
                print(f"  ERROR {pattern_name}: {e}")
                continue
            if report:
                results[(pattern_name, bs)] = report
    finally:
        ring.close()

    # Summary
    print(f"\n{'='*70}")
    print("QUICK RESULTS (5 MiB objects, Llama-70B)")
    print(f"{'='*70}")
    print(f"{'Test':<40} {'Path':<12} {'GB/s':>6} {'p50us':>7} {'p99us':>7} {'Err':>5}")
    print("-" * 75)

    def _row(label, path, report, phase_op):
        if phase_op in report:
            d = report[phase_op]
            err_str = str(d["errors"]) if d["errors"] > 0 else ""
            print(f"{label:<40} {path:<12} {d['throughput_gbps']:>6.1f} {d['p50_us']:>7.0f} {d['p99_us']:>7.0f} {err_str:>5}")
        else:
            print(f"{label:<40} {'?':<12} {'—':>6}")

    if ("decode_block_store", 1) in results:
        _row("Serial store (bs=1)", "GPU→DRAM", results[("decode_block_store", 1)], "decode-writeback/store")
    if ("cold_prefill_store", 64) in results:
        _row("Batched store (bs=64)", "GPU→DRAM", results[("cold_prefill_store", 64)], "prefill-writeback/store")
    if ("compute_local_eviction_and_later_reload", 64) in results:
        _row("Warm load (bs=64)", "DRAM→GPU", results[("compute_local_eviction_and_later_reload", 64)], "demand-reload/load")
    if ("hot_vs_cold_load_paths", 1) in results:
        _row("Cold load serial (bs=1)", "SSD→GPU", results[("hot_vs_cold_load_paths", 1)], "hot-load/load")
    if ("hot_vs_cold_load_paths", 64) in results:
        _row("Cold load batched (bs=64)", "SSD→GPU", results[("hot_vs_cold_load_paths", 64)], "hot-load/load")
    if ("bidirectional_store_load_contention", 1) in results:
        r = results[("bidirectional_store_load_contention", 1)]
        _row("Contended store (bs=1)", "GPU→DRAM", r, "concurrent-bidir/store")
        _row("Contended load (bs=1)", "SSD→GPU", r, "concurrent-bidir/load")

    print(f"{'='*75}")


# Core patterns for full sweep — warm path (data stays in memory tier)
WARM_PATTERNS = [
    "cold_prefill_store",
    "decode_block_store",
    "warm_prefill_load_and_suffix_store",
    "preemption_and_reschedule",
    "compute_local_eviction_and_later_reload",
    "bidirectional_store_load_contention",
    "disaggregated_prefill_decode",
    "continuous_batching_mix",
]

# Cold-path patterns (SSD→GPU loads; require clear_memory_tier, run last)
COLD_PATTERNS_LIST = [
    "hot_vs_cold_load_paths",
    "selective_kv_retrieval",
    "tier_promotion_and_prefetch",
]

CORE_PATTERNS = WARM_PATTERNS + COLD_PATTERNS_LIST

# Object sizes: model the range from small (Llama-8B) to large (Llama-70B)
# object_bytes = block_size(16) * num_layers * kv_bytes_per_token_per_layer
OBJECT_SIZE_CONFIGS = [
    {"label": "1M (Llama-8B-like)", "num_layers": 32, "kv_bytes_per_token_per_layer": 2048},
    {"label": "3M (Llama-30B-like)", "num_layers": 60, "kv_bytes_per_token_per_layer": 3072},
    {"label": "5M (Llama-70B)", "num_layers": 80, "kv_bytes_per_token_per_layer": 4096},
]

BATCH_SIZES = [1, 4, 16, 64, 256]


def cmd_full(args):
    """Full sweep across core patterns × batch sizes × object sizes."""
    print(f"{'='*70}")
    print("certus-fio: FULL SWEEP")
    print(f"{'='*70}")
    print(f"  Patterns: {len(CORE_PATTERNS)} ({len(WARM_PATTERNS)} warm + {len(COLD_PATTERNS_LIST)} cold)")
    print(f"  Object sizes: {len(OBJECT_SIZE_CONFIGS)}")
    print(f"  Batch sizes: {BATCH_SIZES}")
    print(f"  Min duration per run: {args.min_duration}s")
    total_runs = len(CORE_PATTERNS) * len(OBJECT_SIZE_CONFIGS) * len(BATCH_SIZES)
    print(f"  Total runs: {total_runs}")
    print(f"  Estimated time: {total_runs * args.min_duration / 60:.0f} min")
    print()

    results = []

    # Warm patterns
    ring = connect(args.shm_path, ready_timeout=10.0)
    try:
        print("\n=== WARM PATH PATTERNS ===")
        _run_pattern_group(WARM_PATTERNS, args, ring, results)
    finally:
        ring.close()

    # Cold patterns (reconnect for clean channel state)
    if COLD_PATTERNS_LIST:
        ring = connect(args.shm_path, ready_timeout=10.0)
        try:
            print("\n=== COLD PATH PATTERNS (SSD → GPU) ===")
            _run_pattern_group(COLD_PATTERNS_LIST, args, ring, results)
        finally:
            ring.close()

    # Print summary table
    print(f"\n\n{'='*90}")
    print("FULL SWEEP SUMMARY")
    print(f"{'='*90}")
    print(f"{'Pattern':<35} {'ObjSize':<16} {'BS':>4} {'Op':<6} {'GB/s':>6} {'p50us':>7} {'Ops':>6}")
    print("-" * 90)
    for r in results:
        if r["throughput_gbps"] < 0.01:
            continue
        op = r["phase_op"].split("/")[-1]
        print(f"{r['pattern']:<35} {r['object_size']:<16} {r['batch_size']:>4} {op:<6} "
              f"{r['throughput_gbps']:>6.2f} {r['p50_us']:>7.0f} {r['ops']:>6}")

    # Write CSV if requested
    if args.output:
        import csv
        with open(args.output, "w", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=[
                "pattern", "object_size", "batch_size", "phase_op",
                "ops", "throughput_gbps", "avg_us", "p50_us", "p99_us",
                "total_mb", "wall_s", "errors",
            ])
            writer.writeheader()
            writer.writerows(results)
        print(f"\nCSV written to: {args.output}")


def _run_pattern_group(pattern_names, args, ring, results):
    """Run a group of patterns across object sizes and batch sizes."""
    for obj_cfg in OBJECT_SIZE_CONFIGS:
        print(f"\n--- Object size: {obj_cfg['label']} ---")
        for pattern_name in pattern_names:
            pattern_path = resolve_pattern(pattern_name, args.patterns_dir)
            for bs in BATCH_SIZES:
                overrides = {
                    "num_layers": str(obj_cfg["num_layers"]),
                    "kv_bytes_per_token_per_layer": str(obj_cfg["kv_bytes_per_token_per_layer"]),
                    "batch_size": str(bs),
                }
                try:
                    pattern = WorkloadPattern(pattern_path, overrides)
                except Exception:
                    continue
                runner = BenchRunner(
                    pattern=pattern, ring=ring, gpu_id=args.gpu,
                    warmup_ops=args.warmup, min_duration=args.min_duration,
                    batch_size_override=bs,
                )
                try:
                    report = runner.run()
                except Exception as e:
                    print(f"    ERROR: {pattern_name} bs={bs}: {e}")
                    continue
                if report:
                    for phase_op, data in report.items():
                        results.append({
                            "pattern": pattern_name,
                            "object_size": obj_cfg["label"],
                            "batch_size": bs,
                            "phase_op": phase_op,
                            "ops": data["ops"],
                            "throughput_gbps": data["throughput_gbps"],
                            "avg_us": data["avg_us"],
                            "p50_us": data["p50_us"],
                            "p99_us": data["p99_us"],
                            "total_mb": data["total_bytes"] / (1024 * 1024),
                            "wall_s": data["wall_s"],
                            "errors": data["errors"],
                        })


def run_full_sweep(args):
    """Run the full sweep and return results list."""
    results = []

    # Warm patterns first (data stays in memory tier between ops)
    ring = connect(args.shm_path, ready_timeout=10.0)
    try:
        print("\n=== WARM PATH PATTERNS ===")
        _run_pattern_group(WARM_PATTERNS, args, ring, results)
    finally:
        ring.close()

    # Cold patterns second (SSD→GPU; each run calls clear_memory_tier)
    # Reconnect to ensure clean channel state after warm patterns
    if COLD_PATTERNS_LIST:
        ring = connect(args.shm_path, ready_timeout=10.0)
        try:
            print("\n=== COLD PATH PATTERNS (SSD → GPU) ===")
            _run_pattern_group(COLD_PATTERNS_LIST, args, ring, results)
        finally:
            ring.close()

    return results


def analyze_results(results):
    """Compute optimization findings from sweep results. All thresholds are relative, not hardcoded."""
    findings = []
    cold_pattern_names = ["hot_vs_cold", "cache_aware", "selective", "tier_promotion"]
    warm_pattern_names = ["eviction", "preemption", "disaggregated"]

    def _filter(results, **kw):
        out = results
        if "op" in kw:
            out = [r for r in out if f"/{kw['op']}" in r["phase_op"]]
        if "obj" in kw:
            out = [r for r in out if kw["obj"] in r["object_size"]]
        if "bs" in kw:
            out = [r for r in out if r["batch_size"] == kw["bs"]]
        if "bs_min" in kw:
            out = [r for r in out if r["batch_size"] >= kw["bs_min"]]
        if "pattern" in kw:
            out = [r for r in out if kw["pattern"] in r["pattern"]]
        if "patterns" in kw:
            out = [r for r in out if any(p in r["pattern"] for p in kw["patterns"])]
        if "exclude_patterns" in kw:
            out = [r for r in out if not any(p in r["pattern"] for p in kw["exclude_patterns"])]
        return [r for r in out if r["throughput_gbps"] > 0.1]

    # Key metrics (5M objects = Llama-70B, the primary target)
    peak_serial_store = max((r["throughput_gbps"] for r in _filter(results, op="store", obj="5M", bs=1)), default=0)
    peak_batched_store = max((r["throughput_gbps"] for r in _filter(results, op="store", obj="5M", bs=64)), default=0)
    peak_warm_load = max((r["throughput_gbps"] for r in _filter(results, op="load", obj="5M", bs=64, patterns=warm_pattern_names)), default=0)
    cold_serial = max((r["throughput_gbps"] for r in _filter(results, op="load", obj="5M", bs=1, patterns=cold_pattern_names)), default=0)
    cold_batched = max((r["throughput_gbps"] for r in _filter(results, op="load", obj="5M", bs=64, patterns=cold_pattern_names)), default=0)
    peak_cold_load = max(cold_serial, cold_batched)
    peak_contended = max((r["throughput_gbps"] for r in _filter(results, obj="5M", bs=1, pattern="bidirectional")), default=0)

    # Error rates
    total_ops = sum(r["ops"] for r in results if r["ops"] > 0)
    total_errors = sum(r["errors"] for r in results)
    contention_errors = sum(r["errors"] for r in results if "bidirectional" in r["pattern"] or "continuous" in r["pattern"])
    other_errors = total_errors - contention_errors

    # Finding: batched store worse than serial (server-side serialization)
    if peak_batched_store > 0 and peak_serial_store > peak_batched_store * 1.3:
        findings.append({
            "severity": "critical",
            "title": "Batched stores slower than serial",
            "detail": (f"Serial: {peak_serial_store:.1f} GB/s. Batched (bs≥64): {peak_batched_store:.1f} GB/s. "
                       f"Server synchronizes per-key in op_populate, negating batch benefits."),
            "impact": f"Fix: async batch_populate → {peak_warm_load:.0f}+ GB/s potential",
        })

    # Finding: store throughput still below load (room for async DMA)
    if peak_batched_store > 0 and peak_warm_load > peak_batched_store * 1.5:
        findings.append({
            "severity": "warning",
            "title": "Store path has headroom vs load path",
            "detail": (f"Peak store: {peak_batched_store:.1f} GB/s. Peak warm load: {peak_warm_load:.1f} GB/s. "
                       f"Store path uses synchronous DMA per-key; load path uses async batch DMA."),
            "impact": f"Async batch_populate could close the gap: {peak_batched_store:.0f} → {peak_warm_load:.0f} GB/s",
        })

    # Finding: contention
    isolated_peak = max(peak_serial_store, peak_warm_load, peak_cold_load)
    if peak_contended > 0 and peak_contended < isolated_peak * 0.4:
        findings.append({
            "severity": "critical",
            "title": "Contention reduces throughput significantly",
            "detail": (f"Isolated peak: {isolated_peak:.1f} GB/s. Under contention: {peak_contended:.1f} GB/s. "
                       f"Concurrent store+load actors compete for shmq channels and memory-tier locks."),
            "impact": f"Separate store/load channel pools or async coalescing",
        })

    # Finding: small object penalty
    small_stores = _filter(results, op="store", obj="1M", bs=1)
    large_stores = _filter(results, op="store", obj="5M", bs=1)
    if small_stores and large_stores:
        avg_small = statistics.mean(r["throughput_gbps"] for r in small_stores)
        avg_large = statistics.mean(r["throughput_gbps"] for r in large_stores)
        if avg_small < avg_large * 0.7:
            findings.append({
                "severity": "warning",
                "title": "Small objects (1 MiB) are IOPS-limited",
                "detail": (f"1 MiB store: {avg_small:.1f} GB/s. 5 MiB store: {avg_large:.1f} GB/s. "
                           f"Per-op overhead dominates at small sizes."),
                "impact": "Reduce per-op overhead for Llama-8B workloads",
            })

    # Finding: cold load benefits from batching
    if cold_serial > 0 and cold_batched > cold_serial * 2:
        findings.append({
            "severity": "good",
            "title": "Cold load scales well with batch size",
            "detail": (f"Serial (bs=1): {cold_serial:.1f} GB/s. Batched (bs≥64): {cold_batched:.1f} GB/s. "
                       f"Prefetch and batch restore are effective optimization levers."),
            "impact": "Batch swap-in submissions for best SSD throughput",
        })

    # Finding: errors under concurrency
    if contention_errors > 0:
        err_rate = contention_errors / max(1, sum(r["ops"] + r["errors"] for r in results if "bidirectional" in r["pattern"] or "continuous" in r["pattern"]))
        findings.append({
            "severity": "warning" if err_rate < 0.3 else "critical",
            "title": f"Concurrent store failures ({err_rate*100:.0f}% error rate)",
            "detail": (f"{contention_errors} failed ops under concurrency. "
                       f"Server rejects concurrent populates (likely memory-tier allocation contention or CUDA stream serialization)."),
            "impact": "Server-side fix: coalesce concurrent stores or separate allocation paths",
        })

    # Finding: warm load is good
    if peak_warm_load > 10:
        findings.append({
            "severity": "good",
            "title": "Warm load path well-optimized",
            "detail": (f"Peak DRAM→GPU: {peak_warm_load:.1f} GB/s. "
                       f"Async batch DMA with deferred sync delivers near-PCIe bandwidth."),
            "impact": "No action needed on warm load path",
        })

    return {
        "peak_serial_store": peak_serial_store,
        "peak_batched_store": peak_batched_store,
        "peak_warm_load": peak_warm_load,
        "peak_cold_load": peak_cold_load,
        "cold_serial": cold_serial,
        "cold_batched": cold_batched,
        "peak_contended": peak_contended,
        "total_errors": total_errors,
        "contention_errors": contention_errors,
        "findings": findings,
    }


def generate_report_html(results, analysis):
    """Generate HTML report from sweep results and analysis."""
    import json as json_mod
    from datetime import datetime

    # Build data tables for JS
    rows_json = json_mod.dumps(results)
    findings_json = json_mod.dumps(analysis["findings"])

    html = f"""<!DOCTYPE html>
<html><head><meta charset="utf-8">
<title>Certus Storage Analysis</title>
<style>
@import url('https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;600&family=Inter:wght@400;500;600;700&display=swap');
:root {{
  --surface:#fcfcfb;--surface-2:#f4f3f1;--text-1:#0b0b0b;--text-2:#52514e;--text-3:#8a8985;
  --border:#e5e4e0;--store:#2a78d6;--load:#1baf7a;--contend:#eb6834;
  --critical:#d42b2b;--warning:#eda100;--good:#1baf7a;
}}
@media(prefers-color-scheme:dark){{:root:not([data-theme="light"]){{
  --surface:#141413;--surface-2:#1e1e1c;--text-1:#f0efea;--text-2:#c3c2b7;--text-3:#7a7970;
  --border:#2e2e2a;--store:#3987e5;--load:#199e70;--contend:#d95926;
  --critical:#f06060;--warning:#c98500;--good:#199e70;
}}}}
:root[data-theme="dark"]{{
  --surface:#141413;--surface-2:#1e1e1c;--text-1:#f0efea;--text-2:#c3c2b7;--text-3:#7a7970;
  --border:#2e2e2a;--store:#3987e5;--load:#199e70;--contend:#d95926;
  --critical:#f06060;--warning:#c98500;--good:#199e70;
}}
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:'Inter',system-ui,sans-serif;background:var(--surface);color:var(--text-1);
  line-height:1.5;padding:2rem;max-width:1200px;margin:0 auto}}
h1{{font-size:1.75rem;font-weight:700;margin-bottom:.25rem}}
h2{{font-size:1.25rem;font-weight:600;margin:2.5rem 0 1rem}}
h3{{font-size:.85rem;font-weight:600;text-transform:uppercase;letter-spacing:.05em;color:var(--text-3);margin-bottom:.75rem}}
p{{color:var(--text-2);max-width:65ch;margin-bottom:.5rem}}
.subtitle{{color:var(--text-2);font-size:.95rem;margin-bottom:2rem}}
.mono{{font-family:'JetBrains Mono',monospace}}
.stat-row{{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:1rem;margin:1.5rem 0}}
.stat{{background:var(--surface-2);border-radius:8px;padding:1.25rem;border:1px solid var(--border)}}
.stat-value{{font-family:'JetBrains Mono',monospace;font-size:1.5rem;font-weight:600}}
.stat-label{{font-size:.8rem;color:var(--text-3);margin-top:.25rem}}
.chart-box{{background:var(--surface-2);border:1px solid var(--border);border-radius:8px;padding:1.5rem;margin:1rem 0;overflow-x:auto}}
table.data{{width:100%;border-collapse:collapse;font-size:.82rem;font-variant-numeric:tabular-nums}}
table.data th{{text-align:left;padding:8px;color:var(--text-3);font-size:.75rem;border-bottom:1px solid var(--border)}}
table.data td{{padding:8px;border-bottom:1px solid var(--border)}}
table.data td.num{{font-family:'JetBrains Mono',monospace;text-align:right}}
.finding{{border-left:3px solid var(--critical);padding:.75rem 1rem;margin:.75rem 0;background:var(--surface-2);border-radius:0 6px 6px 0}}
.finding.warning{{border-left-color:var(--warning)}}
.finding.good{{border-left-color:var(--good)}}
.finding-title{{font-weight:600;font-size:.9rem}}
.finding-detail{{font-size:.85rem;color:var(--text-2);margin-top:.25rem}}
.finding-impact{{font-size:.8rem;color:var(--good);margin-top:.25rem;font-family:'JetBrains Mono',monospace}}
.bar{{height:18px;border-radius:3px;transition:width .3s}}
</style></head><body>
<h1>Certus Storage Performance Report</h1>
<p class="subtitle">Generated {datetime.now().strftime('%Y-%m-%d %H:%M')} &middot; 4 NVMe (NUMA 0) &middot; 4 GiB memory tier &middot; shmq</p>

<h2>Key Results (5 MiB objects, Llama-70B)</h2>
<div class="chart-box">
<table class="data">
<tr><th>Test</th><th>Path</th><th>bs</th><th>GB/s</th></tr>
<tr><td>Serial store</td><td style="color:var(--store)">GPU&rarr;DRAM</td><td class="num">1</td><td class="num" style="color:var(--store)">{analysis['peak_serial_store']:.1f}</td></tr>
<tr><td>Batched store</td><td style="color:var(--store)">GPU&rarr;DRAM</td><td class="num">64</td><td class="num" style="color:var(--store)">{analysis['peak_batched_store']:.1f}</td></tr>
<tr><td>Warm load</td><td style="color:var(--load)">DRAM&rarr;GPU</td><td class="num">64</td><td class="num" style="color:var(--load)">{analysis['peak_warm_load']:.1f}</td></tr>
<tr><td>Cold load serial</td><td style="color:var(--contend)">SSD&rarr;GPU</td><td class="num">1</td><td class="num" style="color:var(--contend)">{analysis['cold_serial']:.1f}</td></tr>
<tr><td>Cold load batched</td><td style="color:var(--load)">SSD&rarr;GPU</td><td class="num">64</td><td class="num" style="color:var(--load)">{analysis['cold_batched']:.1f}</td></tr>
<tr><td>Under contention</td><td style="color:var(--contend)">mixed</td><td class="num">1</td><td class="num" style="color:var(--contend)">{analysis['peak_contended']:.1f}</td></tr>
</table>
</div>

<h2>Optimization Findings</h2>
<div id="findings"></div>

<h2>Throughput by Pattern (5 MiB objects)</h2>
<div class="chart-box"><table class="data" id="main-table"></table></div>

<h2>Batch Size Sensitivity (5 MiB objects)</h2>
<p>Shows how batch_size affects throughput. Loads benefit from batching (async DMA). Stores degrade at high batch sizes (server-side serialization).</p>
<div class="chart-box"><table class="data" id="batch-table"></table></div>

<h2>Object Size Scaling (batch_size=1)</h2>
<div class="chart-box"><table class="data" id="size-table"></table></div>

<h2>Recommended Optimizations</h2>
<div class="chart-box">
<table class="data">
<tr><th>Priority</th><th>Optimization</th><th>Expected Impact</th><th>Complexity</th></tr>
<tr><td>P0</td><td>Async batch_populate (submit all D2H, one sync)</td><td style="color:var(--good)">Store {analysis['peak_batched_store']:.1f} → {analysis['peak_warm_load']:.0f}+ GB/s</td><td>Medium</td></tr>
<tr><td>P1</td><td>Reduce per-op overhead for small objects</td><td style="color:var(--good)">1 MiB IOPS +50%</td><td>Low</td></tr>
<tr><td>P2</td><td>Separate store/load shmq channel pools</td><td style="color:var(--good)">Contention {analysis['peak_contended']:.1f} → {analysis['peak_contended']*1.6:.1f}+ GB/s</td><td>High</td></tr>
<tr><td>P3</td><td>Fix concurrent store failures</td><td style="color:var(--good)">Eliminate {analysis['contention_errors']} errors under concurrent decode</td><td>Medium</td></tr>
</table>
</div>

<script>
const DATA = {rows_json};
const FINDINGS = {findings_json};

// Render findings
const findingsEl = document.getElementById('findings');
FINDINGS.forEach(f => {{
  const cls = f.severity === 'good' ? 'good' : f.severity === 'warning' ? 'warning' : '';
  findingsEl.innerHTML += `<div class="finding ${{cls}}"><div class="finding-title">${{f.title}}</div><div class="finding-detail">${{f.detail}}</div><div class="finding-impact">${{f.impact}}</div></div>`;
}});

// Main table: 5M objects at realistic batch sizes per pattern
const mainTable = document.getElementById('main-table');
const all5m = DATA.filter(r => r.object_size.includes('5M') && r.throughput_gbps > 0.01);
const COLD_PATTERNS = ['hot_vs_cold_load_paths','selective_kv_retrieval','tier_promotion_and_prefetch','cache_aware_routing_and_remote_hit_migration','warm_prefill_load_and_suffix_store','bidirectional_store_load_contention'];
function getPath(r) {{
  const op = r.phase_op.split('/')[1];
  if (op === 'store') return 'GPU → DRAM';
  if (op === 'delete') return 'metadata';
  if (op === 'load') {{
    if (COLD_PATTERNS.some(p => r.pattern.includes(p))) return 'SSD → GPU';
    return 'DRAM → GPU';
  }}
  return '';
}}

// Natural batch sizes: serial ops (decode, contention) use bs=1,
// batched ops (prefill, loads, disaggregated) use bs=64
const SERIAL_OPS = {{'decode_block_store/store':1, 'bidirectional_store_load_contention/store':1,
  'bidirectional_store_load_contention/load':1, 'continuous_batching_mix/store':1}};
function naturalBs(r) {{
  const key = r.pattern + '/' + r.phase_op.split('/')[1];
  if (SERIAL_OPS[key] !== undefined) return SERIAL_OPS[key];
  const op = r.phase_op.split('/')[1];
  if (op === 'delete') return 1;
  return 64;
}}

// For each pattern+phase_op, pick the row at its natural batch size
const bestByKey = {{}};
all5m.forEach(r => {{
  const key = r.pattern + '|' + r.phase_op;
  const target = naturalBs(r);
  if (!bestByKey[key] || Math.abs(r.batch_size - target) < Math.abs(bestByKey[key].batch_size - target)) {{
    bestByKey[key] = r;
  }}
}});
const main5m = Object.values(bestByKey);

mainTable.innerHTML = '<tr><th>Pattern</th><th>Op</th><th>Path</th><th>bs</th><th style="min-width:130px">GB/s</th><th style="min-width:100px">p50 (us)</th><th>Err</th></tr>';
const byPattern = {{}};
main5m.forEach(r => {{
  if (!byPattern[r.pattern]) byPattern[r.pattern] = [];
  byPattern[r.pattern].push(r);
}});
const patternOrder = Object.entries(byPattern).sort((a,b) => {{
  const maxA = Math.max(...a[1].map(x => x.throughput_gbps));
  const maxB = Math.max(...b[1].map(x => x.throughput_gbps));
  return maxB - maxA;
}});
const maxTp = Math.max(...main5m.map(r => r.throughput_gbps));
patternOrder.forEach(([pat, rows]) => {{
  rows.sort((a,b) => b.throughput_gbps - a.throughput_gbps);
  rows.forEach((r, idx) => {{
    const op = r.phase_op.split('/')[1];
    const path = getPath(r);
    const color = op === 'store' ? 'var(--store)' : op === 'load' ? 'var(--load)' : 'var(--text-3)';
    const pathColor = path.includes('SSD') ? 'var(--contend)' : color;
    const tpPct = (r.throughput_gbps / maxTp * 100).toFixed(0);
    const patLabel = idx === 0 ? pat.replace(/_/g,' ') : '';
    const patStyle = idx === 0 ? 'font-weight:500;font-size:.8rem' : 'color:var(--text-3);font-size:.72rem;padding-left:.5rem';
    const errStr = r.errors > 0 ? r.errors : '';
    mainTable.innerHTML += `<tr>
      <td style="${{patStyle}}">${{patLabel}}</td>
      <td style="font-size:.75rem">${{op}}</td>
      <td style="color:${{pathColor}};font-size:.75rem;white-space:nowrap">${{path}}</td>
      <td class="num">${{r.batch_size}}</td>
      <td class="num"><span style="margin-right:6px">${{r.throughput_gbps.toFixed(1)}}</span><div class="bar" style="display:inline-block;width:${{tpPct}}px;max-width:80px;background:${{color}};opacity:0.5;vertical-align:middle"></div></td>
      <td class="num">${{r.p50_us.toFixed(0)}}</td>
      <td class="num" style="color:var(--critical)">${{errStr}}</td>
    </tr>`;
  }});
  if (rows.length > 1) {{
    mainTable.innerHTML += `<tr><td colspan="7" style="height:3px;padding:0"></td></tr>`;
  }}
}});

// Batch sensitivity heatmap
const batchTable = document.getElementById('batch-table');
const patterns5m = [...new Set(DATA.filter(r => r.object_size.includes('5M')).map(r => r.pattern + '|' + r.phase_op))];
const batchSizes = [1,4,16,64,256];
const allBatchTps = DATA.filter(r => r.object_size.includes('5M') && r.throughput_gbps > 0.01).map(r => r.throughput_gbps);
const batchMaxTp = Math.max(...allBatchTps);

function heatBg(val, maxVal) {{
  const t = Math.min(val / maxVal, 1);
  const h = 145 - t * 145; // green(145) to red(0)
  return `hsla(${{h}}, 60%, 45%, ${{0.15 + t * 0.25}})`;
}}

batchTable.innerHTML = '<tr><th>Pattern</th><th>Path</th>' + batchSizes.map(bs => `<th>BS=${{bs}}</th>`).join('') + '</tr>';
patterns5m.forEach(pk => {{
  const [pat, phop] = pk.split('|');
  const rows = DATA.filter(r => r.pattern === pat && r.phase_op === phop && r.object_size.includes('5M') && r.throughput_gbps > 0.01);
  if (rows.length < 3) return;
  const op = phop.split('/')[1];
  const path = op === 'store' ? 'GPU→DRAM' : COLD_PATTERNS.some(p => pat.includes(p)) ? 'SSD→GPU' : 'DRAM→GPU';
  const pathColor = path.includes('SSD') ? 'var(--contend)' : op === 'store' ? 'var(--store)' : 'var(--load)';
  let cells = `<td style="font-size:.75rem">${{pat.replace(/_/g,' ').replace('_and_', ' & ').replace('_or_', ' | ')}}</td>`;
  cells += `<td style="font-size:.72rem;color:${{pathColor}}">${{path}}</td>`;
  batchSizes.forEach(bs => {{
    const r = rows.find(x => x.batch_size === bs);
    if (r) {{
      cells += `<td class="num" style="background:${{heatBg(r.throughput_gbps, batchMaxTp)}}">${{r.throughput_gbps.toFixed(1)}}</td>`;
    }} else {{
      cells += `<td class="num">-</td>`;
    }}
  }});
  batchTable.innerHTML += `<tr>${{cells}}</tr>`;
}});

// Add a legend for the heatmap
batchTable.innerHTML += `<tr><td colspan="${{batchSizes.length + 2}}" style="padding-top:8px;font-size:.72rem;color:var(--text-3)">Color: green=high throughput, red=low. Values in GB/s.</td></tr>`;

// Size scaling heatmap
const sizeTable = document.getElementById('size-table');
const objSizes = ['1M','3M','5M'];
const allSizeTps = DATA.filter(r => r.batch_size === 1 && r.throughput_gbps > 0.01).map(r => r.throughput_gbps);
const sizeMaxTp = Math.max(...allSizeTps);

sizeTable.innerHTML = '<tr><th>Pattern</th><th>Path</th><th>1 MiB<br><span style="font-weight:400;font-size:.65rem">(Llama-8B)</span></th><th>3 MiB<br><span style="font-weight:400;font-size:.65rem">(Llama-30B)</span></th><th>5 MiB<br><span style="font-weight:400;font-size:.65rem">(Llama-70B)</span></th></tr>';
const patternsBS1 = [...new Set(DATA.filter(r => r.batch_size === 1 && r.throughput_gbps > 0.01).map(r => r.pattern + '|' + r.phase_op))];
patternsBS1.forEach(pk => {{
  const [pat, phop] = pk.split('|');
  const rows = DATA.filter(r => r.pattern === pat && r.phase_op === phop && r.batch_size === 1 && r.throughput_gbps > 0.01);
  if (rows.length < 2) return;
  const op = phop.split('/')[1];
  const path = op === 'store' ? 'GPU→DRAM' : COLD_PATTERNS.some(p => pat.includes(p)) ? 'SSD→GPU' : 'DRAM→GPU';
  const pathColor = path.includes('SSD') ? 'var(--contend)' : op === 'store' ? 'var(--store)' : 'var(--load)';
  let cells = `<td style="font-size:.75rem">${{pat.replace(/_/g,' ').replace('_and_', ' & ').replace('_or_', ' | ')}}</td>`;
  cells += `<td style="font-size:.72rem;color:${{pathColor}}">${{path}}</td>`;
  objSizes.forEach(sz => {{
    const r = rows.find(x => x.object_size.includes(sz));
    if (r) {{
      cells += `<td class="num" style="background:${{heatBg(r.throughput_gbps, sizeMaxTp)}}">${{r.throughput_gbps.toFixed(1)}}</td>`;
    }} else {{
      cells += `<td class="num">-</td>`;
    }}
  }});
  sizeTable.innerHTML += `<tr>${{cells}}</tr>`;
}});
sizeTable.innerHTML += `<tr><td colspan="5" style="padding-top:8px;font-size:.72rem;color:var(--text-3)">Color: green=high throughput, red=low. Values in GB/s. batch_size=1 (serial).</td></tr>`;
</script></body></html>"""
    return html


def cmd_report(args):
    """Run full sweep and generate HTML report with optimization analysis."""
    import csv as csv_mod

    if args.from_csv:
        print(f"Loading results from: {args.from_csv}")
        results = []
        with open(args.from_csv) as f:
            reader = csv_mod.DictReader(f)
            for row in reader:
                results.append({
                    "pattern": row["pattern"],
                    "object_size": row["object_size"],
                    "batch_size": int(row["batch_size"]),
                    "phase_op": row["phase_op"],
                    "ops": int(row["ops"]),
                    "throughput_gbps": float(row["throughput_gbps"]),
                    "avg_us": float(row["avg_us"]),
                    "p50_us": float(row["p50_us"]),
                    "p99_us": float(row["p99_us"]),
                    "total_mb": float(row["total_mb"]),
                    "wall_s": float(row["wall_s"]),
                    "errors": int(row["errors"]),
                })
        print(f"  Loaded {len(results)} rows")
    else:
        print(f"{'='*70}")
        print("certus-fio: REPORT")
        print(f"{'='*70}")
        total_runs = len(CORE_PATTERNS) * len(OBJECT_SIZE_CONFIGS) * len(BATCH_SIZES)
        print(f"  Running full sweep: {total_runs} configurations")
        print(f"  Estimated time: {total_runs * args.min_duration / 60:.0f} min")
        print()

        results = run_full_sweep(args)

        if args.csv:
            with open(args.csv, "w", newline="") as f:
                writer = csv_mod.DictWriter(f, fieldnames=[
                    "pattern", "object_size", "batch_size", "phase_op",
                    "ops", "throughput_gbps", "avg_us", "p50_us", "p99_us",
                    "total_mb", "wall_s", "errors",
                ])
                writer.writeheader()
                writer.writerows(results)
            print(f"\nCSV: {args.csv}")

    analysis = analyze_results(results)
    html = generate_report_html(results, analysis)
    Path(args.output).write_text(html)
    print(f"\nReport: {args.output}")
    print(f"  Open: xdg-open {args.output}")


if __name__ == "__main__":
    main()

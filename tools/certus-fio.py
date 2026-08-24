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

# ── Locate shmq helpers (sibling to this script's repo) ──
_THIS_DIR = Path(__file__).resolve().parent
_APPS_PYTHON = _THIS_DIR.parent / "apps" / "python"
if _APPS_PYTHON.exists():
    sys.path.insert(0, str(_APPS_PYTHON))

from certus_shmq_helpers import (
    Ring,
    RingError,
    add_shm_arg,
    connect,
    single_region,
)

PATTERNS_DIR = _THIS_DIR.parent / "knowledge" / "workload_patterns"

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
        return self.wall_end - self.wall_start

    @property
    def throughput_gbps(self):
        return (self.total_bytes / self.elapsed / 1e9) if self.elapsed > 0 else 0


class BenchRunner:
    def __init__(self, pattern: WorkloadPattern, ring: Ring, gpu_id: int = 0,
                 cleanup_before: bool = False):
        self.pattern = pattern
        self.ring = ring
        self.gpu_id = gpu_id
        self.cleanup_before = cleanup_before
        self._stop = threading.Event()
        self._all_keys = set()
        self._gpu_buffers = []
        self._key_cache = {}
        self._key_base = random.randint(1_000_000, 50_000_000)

        _libcudart.cudaSetDevice(gpu_id)

    def _alloc_buffer(self, size):
        ptr, handle_bytes = cuda_alloc(size)
        self._gpu_buffers.append(ptr)
        return handle_bytes, size

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
                    count = eval_expr(phase.get("actors", {}).get("count", 1), self.pattern.params)
                    max_a = max(max_a, count)
        return max_a

    def _setup_preconditions(self):
        for pc in self.pattern.preconditions:
            ks_name = pc["subject"]
            state = pc["state"]
            ks = self.pattern.keyspaces[ks_name]

            if state == "present_in_store" and pc["value"]:
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
                time.sleep(0.5)

            if state == "absent_from_local_cache" and pc["value"]:
                print(f"  Setup: clearing memory tier")
                self.ring.clear_memory_tier()

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

        ops_sequence: list of (op, ks_name, object_bytes, repeat_count, order, result)
        For multi-op phases, same actor runs ops sequentially (not split across actors).
        """
        _libcudart.cudaSetDevice(self.gpu_id)

        # Pre-allocate one buffer per op type (one buffer per actor is correct for shmq
        # since Ring is blocking — no async pipelining within one channel)
        import itertools
        op_buffers = {}
        for op, ks_name, object_bytes, _, _, _ in ops_sequence:
            if op in ("store", "load") and op not in op_buffers:
                op_buffers[op] = self._alloc_buffer(object_bytes)

        ready_latch.append(True)
        start_event.wait()

        concurrency_sem.acquire()
        try:
            for op, ks_name, object_bytes, repeat_count, order, result in ops_sequence:
                if self._stop.is_set():
                    break

                base_keys = list(self._get_keys(ks_name, actor_id, num_actors))
                if repeat_count > 1 and repeat_count != len(base_keys):
                    keys = list(itertools.islice(itertools.cycle(base_keys), repeat_count))
                else:
                    keys = base_keys[:]
                if order == "random":
                    random.shuffle(keys)

                if op == "store":
                    handle_bytes, size = op_buffers["store"]
                    region = single_region(handle_bytes, self.gpu_id, size)
                    for key in keys:
                        if self._stop.is_set():
                            break
                        t0 = time.perf_counter()
                        try:
                            oks = self.ring.populate([(key, [region])])
                            t1 = time.perf_counter()
                            if oks and oks[0]:
                                result.record(t1 - t0, object_bytes)
                            else:
                                result.record_error()
                        except RingError:
                            result.record_error()
                        self._all_keys.add(key)

                elif op == "load":
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
                param_name = f"{op}_actors"
                op_actors = max(1, int(self.pattern.params.get(param_name, total_actors // len(operations))))

                for i in range(op_actors):
                    ops_seq = [(op, ks_name, object_bytes, repeat_count, order, op_results[op])]
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
                ops_seq_template.append((op, ks_name, object_bytes, repeat_count, order, op_results[op]))

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
            self._setup_preconditions()
            all_results = []
            for phase in self.pattern.phases:
                print(f"\n  Phase: {phase['id']}")
                phase_results = self._run_phase(phase)
                all_results.extend(phase_results)
                if phase.get("barrier_after", False):
                    self.ring.flush_to_ssd()

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

    args = parser.parse_args()
    if args.command == "list":
        cmd_list(args)
    elif args.command == "describe":
        cmd_describe(args)
    elif args.command == "run":
        cmd_run(args)


if __name__ == "__main__":
    main()

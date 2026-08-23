#!/usr/bin/env python3
"""certus-fio — FIO-like pattern-driven benchmark for certus gRPC storage.

Reads workload pattern YAML files and executes them against a running
certus-server. Each pattern defines keyspaces, preconditions, phases with
store/load/delete operations, actor counts, and measurement targets.

Usage:
    python3 certus-fio.py list
    python3 certus-fio.py describe --pattern cold_prefill_store
    python3 certus-fio.py run --pattern cold_prefill_store --server localhost:50051
    python3 certus-fio.py run --pattern bidirectional_store_load_contention --override store_actors=2 load_actors=8
"""

import argparse
import ctypes
import math
import os
import random
import signal
import statistics
import sys
import threading
import time
from pathlib import Path

import grpc
import yaml

# ── Proto imports ──
PROTO_PATHS = [
    Path("/home/nara/certus/evo-connector/ai-native-storage-certus/apps/python"),
    Path("/home/nara/certus/main/ai-native-storage-certus/apps/python"),
    Path("/home/nara/certus/evo-connector/ai-native-storage-certus/certus-grpc-connector/certus_grpc_connector"),
]
for p in PROTO_PATHS:
    if p.exists():
        sys.path.insert(0, str(p))
        break

import dispatcher_pb2
import dispatcher_pb2_grpc

PATTERNS_DIR = Path(__file__).resolve().parent.parent / "knowledge" / "workload_patterns"

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
                else:
                    print(f"  WARNING: unknown override '{k}' (available: {list(self.params.keys())})")

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
        self.measure = raw.get("measure", [])
        self.expected_io = raw.get("expected_io", {})

    def describe(self):
        print(f"Pattern: {self.id}")
        print(f"  Name: {self.name}")
        print(f"  Status: {self.status}")
        print(f"  Parameters:")
        for k, v in self.params.items():
            print(f"    {k} = {v}")
        print(f"  Keyspaces:")
        for ks_name, ks in self.keyspaces.items():
            print(f"    {ks_name}: {ks['cardinality']} objects × {ks['object_bytes']} bytes ({ks['sharing']})")
        print(f"  Preconditions:")
        for pc in self.preconditions:
            print(f"    {pc['subject']}: {pc['state']} = {pc['value']}")
        print(f"  Phases:")
        for phase in self.phases:
            actors = phase.get("actors", {})
            count = eval_expr(actors.get("count", 1), self.params)
            ops = [op["op"] for op in phase.get("operations", [])]
            barrier = phase.get("barrier_after", False)
            print(f"    {phase['id']}: {count} actors, ops={ops}, barrier={barrier}")
        print(f"  Expected IO:")
        for k, v in self.expected_io.items():
            try:
                print(f"    {k}: {eval_expr(v, self.params)}")
            except Exception:
                print(f"    {k}: {v} (cannot evaluate)")


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
    def __init__(self, pattern: WorkloadPattern, server: str, gpu_id: int = 0,
                 pipeline_depth: int = 4, cleanup_before: bool = False):
        self.pattern = pattern
        self.server = server
        self.gpu_id = gpu_id
        self.pipeline_depth = pipeline_depth
        self.cleanup_before = cleanup_before
        self._stop = threading.Event()
        self._all_keys = set()
        self._gpu_buffers = []
        self._key_cache = {}  # (ks_name, actor_id) → list of keys

        _libcudart.cudaSetDevice(gpu_id)

        self.channel = grpc.insecure_channel(server, options=[
            ("grpc.max_send_message_length", 256 * 1024 * 1024),
            ("grpc.max_receive_message_length", 256 * 1024 * 1024),
        ])
        self.stub = dispatcher_pb2_grpc.DispatcherStub(self.channel)

        # FIX #1: Generate stable key base ONCE per run
        self._key_base = random.randint(1_000_000, 50_000_000)

    def _alloc_buffer(self, size):
        ptr, handle_bytes = cuda_alloc(size)
        self._gpu_buffers.append(ptr)
        return dispatcher_pb2.IpcHandle(cuda_ipc_handle=handle_bytes, size=size, gpu_device_id=self.gpu_id)

    def _get_keys(self, ks_name, actor_id, num_actors):
        """FIX #1: Return consistent keys for a keyspace+actor. Cached across phases."""
        cache_key = (ks_name, actor_id)
        if cache_key in self._key_cache:
            return self._key_cache[cache_key]

        ks = self.pattern.keyspaces[ks_name]
        cardinality = ks["cardinality"]
        # Each keyspace gets its own offset to avoid collisions
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

    def _setup_preconditions(self):
        for pc in self.pattern.preconditions:
            ks_name = pc["subject"]
            state = pc["state"]
            ks = self.pattern.keyspaces[ks_name]

            if state == "present_in_store" and pc["value"]:
                # Populate all keys for this keyspace (all actors)
                print(f"  Setup: populating {ks_name} ({ks['cardinality']} × {ks['object_bytes']} bytes)")
                # For global keyspace, populate once. For per_actor, populate for each actor.
                if ks["sharing"] == "global":
                    keys = self._get_keys(ks_name, 0, 1)
                    self._populate_keys_batch(keys, ks["object_bytes"])
                else:
                    # Determine how many actors will use this keyspace
                    max_actors = self._max_actors_for_keyspace(ks_name)
                    for aid in range(max_actors):
                        keys = self._get_keys(ks_name, aid, max_actors)
                        self._populate_keys_batch(keys, ks["object_bytes"])

                # Ensure on SSD
                resp = self.stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())
                time.sleep(0.5)

            if state == "absent_from_local_cache" and pc["value"]:
                print(f"  Setup: clearing memory tier (force cold path for {ks_name})")
                self.stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())

            if state == "absent_from_store" and pc["value"]:
                # Ensure keys don't exist — remove if they do
                if ks["sharing"] == "global":
                    keys = self._get_keys(ks_name, 0, 1)
                else:
                    max_actors = self._max_actors_for_keyspace(ks_name)
                    keys = []
                    for aid in range(max_actors):
                        keys.extend(self._get_keys(ks_name, aid, max_actors))
                for batch_start in range(0, len(keys), 100):
                    try:
                        self.stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=keys[batch_start:batch_start+100]))
                    except grpc.RpcError:
                        pass

    def _max_actors_for_keyspace(self, ks_name):
        """Find the maximum actor count across all phases that use this keyspace."""
        max_a = 1
        for phase in self.pattern.phases:
            for op in phase.get("operations", []):
                if op.get("keys") == ks_name:
                    actors_spec = phase.get("actors", {})
                    count = eval_expr(actors_spec.get("count", 1), self.pattern.params)
                    max_a = max(max_a, count)
        return max_a

    def _populate_keys_batch(self, keys, object_bytes):
        ipc = self._alloc_buffer(object_bytes)
        for key in keys:
            if self._stop.is_set():
                return
            entries = [dispatcher_pb2.PopulateEntry(key=key, ipc_handle=ipc)]
            self.stub.Populate(dispatcher_pb2.BatchPopulateRequest(entries=entries))

    def _run_actor(self, actor_id, op, ks_name, num_actors, object_bytes, repeat_count,
                   order, result: PhaseResult, start_event: threading.Event,
                   concurrency_sem: threading.Semaphore, ready_latch: list):
        """Each actor runs independently with own buffer."""
        _libcudart.cudaSetDevice(self.gpu_id)

        base_keys = list(self._get_keys(ks_name, actor_id, num_actors))

        # FIX #1 (repeat): Cycle through keys repeat_count times, not duplicate list
        if repeat_count > 1 and repeat_count != len(base_keys):
            # repeat = total operations requested; cycle keys to fill
            import itertools
            keys = list(itertools.islice(itertools.cycle(base_keys), repeat_count))
        else:
            keys = base_keys[:]

        # Apply order (on a copy, not the cached original)
        if order == "random":
            random.shuffle(keys)

        # Allocate SEPARATE buffers per actor
        if op == "load":
            ipc_handles = [self._alloc_buffer(object_bytes) for _ in range(self.pipeline_depth)]
        elif op == "delete":
            ipc_handles = []
        else:
            ipc_handles = [self._alloc_buffer(object_bytes)]

        # Signal ready and wait for all actors to be ready before starting
        ready_latch.append(True)
        start_event.wait()

        # Acquire concurrency semaphore (limits how many actors run simultaneously)
        concurrency_sem.acquire()
        try:
            self._run_actor_work(op, keys, ipc_handles, object_bytes, result)
        finally:
            concurrency_sem.release()

    def _run_actor_work(self, op, keys, ipc_handles, object_bytes, result):
        """Inner work loop — separated for semaphore wrapping."""
        if op == "store":
            ipc = ipc_handles[0]
            in_flight = []
            for key in keys:
                if self._stop.is_set():
                    break
                entries = [dispatcher_pb2.PopulateEntry(key=key, ipc_handle=ipc)]
                req = dispatcher_pb2.BatchPopulateRequest(entries=entries)
                t0 = time.perf_counter()
                future = self.stub.Populate.future(req)
                in_flight.append((future, t0))
                self._all_keys.add(key)

                while len(in_flight) >= self.pipeline_depth:
                    f, t_sub = in_flight.pop(0)
                    try:
                        resp = f.result()
                        t1 = time.perf_counter()
                        if resp.results and resp.results[0].success:
                            result.record(t1 - t_sub, object_bytes)
                        else:
                            result.record_error()
                    except grpc.RpcError:
                        result.record_error()

            for f, t_sub in in_flight:
                try:
                    resp = f.result()
                    t1 = time.perf_counter()
                    if resp.results and resp.results[0].success:
                        result.record(t1 - t_sub, object_bytes)
                    else:
                        result.record_error()
                except grpc.RpcError:
                    result.record_error()

        elif op == "load":
            in_flight = []
            slot = 0
            for key in keys:
                if self._stop.is_set():
                    break
                ipc = ipc_handles[slot % self.pipeline_depth]
                entries = [dispatcher_pb2.LookupEntry(key=key, ipc_handle=ipc)]
                req = dispatcher_pb2.BatchLookupRequest(entries=entries)
                t0 = time.perf_counter()
                future = self.stub.Lookup.future(req)
                in_flight.append((future, t0))
                slot += 1

                while len(in_flight) >= self.pipeline_depth:
                    f, t_sub = in_flight.pop(0)
                    try:
                        resp = f.result()
                        _libcudart.cudaDeviceSynchronize()
                        t1 = time.perf_counter()
                        if resp.results and resp.results[0].success:
                            result.record(t1 - t_sub, object_bytes)
                        else:
                            result.record_error()
                    except grpc.RpcError:
                        result.record_error()

            for f, t_sub in in_flight:
                try:
                    resp = f.result()
                    _libcudart.cudaDeviceSynchronize()
                    t1 = time.perf_counter()
                    if resp.results and resp.results[0].success:
                        result.record(t1 - t_sub, object_bytes)
                    else:
                        result.record_error()
                except grpc.RpcError:
                    result.record_error()

        elif op == "delete":
            for batch_start in range(0, len(keys), 100):
                if self._stop.is_set():
                    break
                batch_keys = keys[batch_start:batch_start + 100]
                t0 = time.perf_counter()
                try:
                    self.stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=batch_keys))
                    t1 = time.perf_counter()
                    result.record(t1 - t0, 0)
                except grpc.RpcError:
                    result.record_error()

    def _run_phase(self, phase_spec) -> list:
        """Execute phase with proper actor assignment and concurrency control."""
        phase_id = phase_spec["id"]
        actors_spec = phase_spec.get("actors", {})
        operations = phase_spec.get("operations", [])
        total_actors = max(1, eval_expr(actors_spec.get("count", 1), self.pattern.params))
        concurrency = max(1, eval_expr(actors_spec.get("concurrency", total_actors), self.pattern.params))

        results = []
        threads = []

        # FIX #3: Split actors between operations
        if len(operations) > 1:
            actors_per_op = self._split_actors(operations, total_actors)
        else:
            actors_per_op = [total_actors]

        # FIX #2: Use Event for synchronized start + Semaphore for concurrency limit
        start_event = threading.Event()
        concurrency_sem = threading.Semaphore(concurrency)
        ready_latch = []  # actors append True when ready

        actor_offset = 0
        total_thread_count = 0
        for op_idx, op_spec in enumerate(operations):
            op = op_spec["op"]
            ks_name = op_spec["keys"]
            ks = self.pattern.keyspaces[ks_name]
            object_bytes = ks["object_bytes"]
            repeat_count = eval_expr(op_spec.get("repeat", 1), self.pattern.params) if "repeat" in op_spec else 1
            order = op_spec.get("order", "sequential")
            op_actors = actors_per_op[op_idx]

            result = PhaseResult(phase_id, op)
            results.append(result)

            # Create ALL actor threads (semaphore limits concurrent execution)
            for i in range(op_actors):
                t = threading.Thread(
                    target=self._run_actor,
                    args=(actor_offset + i, op, ks_name, total_actors, object_bytes,
                          repeat_count, order, result, start_event, concurrency_sem, ready_latch),
                    daemon=True,
                )
                threads.append(t)
                total_thread_count += 1
            actor_offset += op_actors

        # Start all threads (they'll wait on start_event after allocating buffers)
        for t in threads:
            t.start()

        # Wait until all actors are ready (allocated buffers, waiting at start_event)
        deadline = time.time() + 30
        while len(ready_latch) < total_thread_count and time.time() < deadline:
            time.sleep(0.01)

        # FIX #6 (timing): Record wall_start AFTER all actors ready, BEFORE releasing them
        wall_start = time.perf_counter()
        for r in results:
            r.wall_start = wall_start

        # Release all actors simultaneously
        start_event.set()

        # Wait for completion
        for t in threads:
            t.join()

        wall_end = time.perf_counter()
        for r in results:
            r.wall_end = wall_end

        return results

    def _split_actors(self, operations, total_actors):
        """Split actors between operations in a multi-op phase.

        Rules:
        - If parameters like store_actors/load_actors exist, use them
        - If total_actors == 1 (or very small), all ops share the same actor pool
          (they run sequentially on the same actors — like load-then-delete)
        - Otherwise split evenly
        """
        # Try to infer from parameter names (e.g., store_actors, load_actors)
        splits = []
        for op_spec in operations:
            op = op_spec["op"]
            param_name = f"{op}_actors"
            if param_name in self.pattern.params:
                splits.append(max(1, int(self.pattern.params[param_name])))
            else:
                splits.append(None)

        if all(s is not None for s in splits):
            return splits

        # If total actors is small or equal to num ops, each op gets all actors
        # (they execute sequentially on the same actors — e.g., load then delete)
        if total_actors <= len(operations):
            return [max(1, total_actors)] * len(operations)

        # Default: split evenly, minimum 1 per op
        per_op = max(1, total_actors // len(operations))
        result = [per_op] * len(operations)
        # Give remainder to first op
        result[0] += total_actors - sum(result)
        return result

    def run(self) -> dict:
        print(f"\n{'='*70}")
        print(f"certus-fio: {self.pattern.id}")
        print(f"{'='*70}")
        print(f"  Server: {self.server}")
        print(f"  GPU: {self.gpu_id}")
        print(f"  Pipeline depth: {self.pipeline_depth}")
        print(f"  Pattern: {self.pattern.name}")
        for k, v in self.pattern.params.items():
            print(f"  {k}: {v}")
        for ks_name, ks in self.pattern.keyspaces.items():
            total_mb = ks["cardinality"] * ks["object_bytes"] / (1024 * 1024)
            print(f"  {ks_name}: {ks['cardinality']} × {ks['object_bytes']//1024}KB = {total_mb:.1f} MB")
        print()

        if self.cleanup_before:
            print("  Cleanup: clearing memory tier...")
            self.stub.ClearMemoryTier(dispatcher_pb2.ClearMemoryTierRequest())

        try:
            self._setup_preconditions()

            all_results = []
            for phase in self.pattern.phases:
                phase_id = phase["id"]
                print(f"\n  Phase: {phase_id}")
                phase_results = self._run_phase(phase)
                all_results.extend(phase_results)

                if phase.get("barrier_after", False):
                    try:
                        self.stub.FlushToSsd(dispatcher_pb2.FlushToSsdRequest())
                    except grpc.RpcError:
                        pass

            # Report
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
            # FIX #11: Cleanup in finally block
            print(f"\n  Cleanup: removing {len(self._all_keys)} keys...")
            all_keys = list(self._all_keys)
            for batch_start in range(0, len(all_keys), 100):
                try:
                    self.stub.Remove(dispatcher_pb2.BatchRemoveRequest(keys=all_keys[batch_start:batch_start+100]))
                except grpc.RpcError:
                    pass
            for ptr in self._gpu_buffers:
                cuda_free(ptr)
            self.channel.close()

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
            name = doc.get("name", doc.get("id", "?"))
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

    runner = BenchRunner(
        pattern=pattern, server=args.server, gpu_id=args.gpu,
        pipeline_depth=args.pipeline_depth, cleanup_before=args.cleanup_before,
    )

    def sighandler(sig, frame):
        print("\n  Interrupted — cleaning up...")
        runner.stop()

    signal.signal(signal.SIGINT, sighandler)
    signal.signal(signal.SIGTERM, sighandler)
    runner.run()


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
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("list", help="List available patterns")

    p_desc = subparsers.add_parser("describe", help="Describe a pattern")
    p_desc.add_argument("--pattern", required=True)
    p_desc.add_argument("--override", nargs="*", help="key=value overrides")

    p_run = subparsers.add_parser("run", help="Run a benchmark")
    p_run.add_argument("--pattern", required=True)
    p_run.add_argument("--server", default="localhost:50051")
    p_run.add_argument("--gpu", type=int, default=0)
    p_run.add_argument("--pipeline-depth", type=int, default=4)
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

#!/usr/bin/env python3
"""
Certus KVCache Storage Benchmark

Replays Mooncake FAST25 inference traces against disk or Certus backends
using identical workload patterns and metrics for apples-to-apples comparison.

Based on Mooncake storage_benchmark_v1; adds --backend certus for exercising
Certus via shmq with the same trace-replay engine.
"""

import argparse
import json
import sys
import time
import statistics
from contextlib import ExitStack
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path
from typing import List, Dict, Any

from storage import DiskHashTable
from layout import get_model_config, create_layout


# ============================================================================
# Data Structures
# ============================================================================


@dataclass
class KVCacheRequest:
    """KVCache request from trace"""

    timestamp: float
    hash_ids: List[int]
    input_length: int
    output_length: int


# ============================================================================
# Trace Replay
# ============================================================================


class TraceReplay:
    """Trace replay handler"""

    def __init__(self, trace_path: str):
        self.trace_path = trace_path

    def load_all(self) -> List[KVCacheRequest]:
        """Load all requests from trace file"""
        requests = []
        with open(self.trace_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line:
                    req = json.loads(line)
                    requests.append(
                        KVCacheRequest(
                            timestamp=req.get("timestamp", 0),
                            hash_ids=req.get("hash_ids", []),
                            input_length=req.get("input_length", 0),
                            output_length=req.get("output_length", 0),
                        )
                    )
        return requests


# ============================================================================
# Storage Factory
# ============================================================================


def create_storage(backend, **kwargs):
    """Create a storage backend instance.

    Args:
        backend: 'disk' or 'certus'
        **kwargs: Backend-specific arguments.
            disk: storage_dir, page_size, max_pages, file_mode, fsync_mode, fsync_batch_size
            certus: page_size, max_pages, shm_path, gpu_device
    """
    if backend == "disk":
        return DiskHashTable(
            storage_dir=kwargs["storage_dir"],
            page_size=kwargs["page_size"],
            max_pages=kwargs["max_pages"],
            file_mode=kwargs.get("file_mode", "single"),
            fsync_mode=kwargs.get("fsync_mode", "none"),
            fsync_batch_size=kwargs.get("fsync_batch_size", 100),
        )
    elif backend == "certus":
        from storage import get_certus_storage_class
        CertusStorage = get_certus_storage_class()
        return CertusStorage(
            page_size=kwargs["page_size"],
            max_pages=kwargs["max_pages"],
            shm_path=kwargs.get("shm_path", "/dev/shm/certus-shmq"),
            gpu_device=kwargs.get("gpu_device", 0),
        )
    else:
        raise ValueError(f"Unknown backend: {backend}")


# ============================================================================
# Storage Benchmark
# ============================================================================


class StorageBenchmark:
    """KVCache storage benchmark

    Processes KVCache requests using layout-generated access patterns.
    """

    def __init__(
        self,
        storage,
        model_config: dict,
        page_size_tokens: int = 512,
    ):
        """Initialize benchmark

        Args:
            storage: Storage backend instance (DiskHashTable or CertusStorage)
            model_config: Model configuration dict
            page_size_tokens: Tokens per page (default: 512)
        """
        self.model_config = model_config
        self.layout = create_layout(model_config, page_size_tokens)
        self.page_size_bytes = self.layout.value_size_bytes
        self.storage = storage

        # Statistics
        self.stats = {
            "total_requests": 0,
            "total_tokens": 0,
            "read_pages": 0,
            "write_pages": 0,
            "page_hits": 0,
            "request_io_latencies_ms": [],
            "request_wall_latencies_ms": [],
        }

    def process_request(self, req: KVCacheRequest) -> float:
        """Process a KVCache request

        Args:
            req: KVCache request

        Returns:
            Total latency in milliseconds
        """
        self.stats["total_requests"] += 1
        self.stats["total_tokens"] += req.input_length + req.output_length

        request_start = time.perf_counter()
        io_latency_ms = 0.0

        # Process each access requirement from layout
        for access in self.layout.get_operations(req):
            if self.storage.exists(access.page_id):
                # Page exists, perform READ
                latency = self.storage.read(
                    access.page_id,
                    offset_in_page=access.offset_in_page,
                    length=access.length,
                )
                if latency is None:
                    continue
                io_latency_ms += latency
                self.stats["read_pages"] += 1
                self.stats["page_hits"] += 1
            else:
                # Page doesn't exist, perform WRITE
                latency = self.storage.write(
                    access.page_id,
                    offset_in_page=access.offset_in_page,
                    length=access.length,
                )
                if latency is None:
                    continue
                io_latency_ms += latency
                self.stats["write_pages"] += 1

        wall_latency_ms = (time.perf_counter() - request_start) * 1000.0
        self.stats["request_io_latencies_ms"].append(io_latency_ms)
        self.stats["request_wall_latencies_ms"].append(wall_latency_ms)
        return io_latency_ms

    def get_stats(self) -> Dict:
        """Get statistics"""
        storage_stats = self.storage.get_stats()
        request_io_latencies = self.stats["request_io_latencies_ms"]
        request_wall_latencies = self.stats["request_wall_latencies_ms"]

        total_pages = self.stats["read_pages"] + self.stats["write_pages"]

        return {
            "total_requests": self.stats["total_requests"],
            "total_tokens": self.stats["total_tokens"],
            "total_pages": total_pages,
            "read_pages": self.stats["read_pages"],
            "write_pages": self.stats["write_pages"],
            "page_hits": self.stats["page_hits"],
            "page_hit_rate": self.stats["read_pages"] / total_pages
            if total_pages > 0
            else 0,
            "write_ratio": self.stats["write_pages"] / total_pages
            if total_pages > 0
            else 0,
            "request_io_latency": latency_stats(request_io_latencies),
            "request_wall_latency": latency_stats(request_wall_latencies),
            "storage": storage_stats,
        }

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False

    def close(self, force_sync: bool = True):
        self.storage.close(force_sync=force_sync)


# ============================================================================
# Benchmark Runner
# ============================================================================


def get_max_page_id(requests: List[KVCacheRequest]) -> int:
    max_id = 0
    for req in requests:
        if req.hash_ids:
            max_id = max(max_id, max(req.hash_ids))
    return max_id


def parse_csv_floats(value: str) -> List[float]:
    return [float(item.strip()) for item in value.split(",") if item.strip()]


def wait_for_replay_time(
    req: KVCacheRequest, base_timestamp: float, start_time: float, replay_scale: float
):
    if replay_scale <= 0 or req.timestamp == 0:
        return
    target_time = start_time + max(0.0, req.timestamp - base_timestamp) / (
        1000.0 * replay_scale
    )
    delay = target_time - time.perf_counter()
    if delay > 0:
        time.sleep(delay)


def latency_stats(values: List[float]) -> Dict[str, float]:
    if not values:
        return {"avg_ms": 0, "p50_ms": 0, "p95_ms": 0, "p99_ms": 0}

    sorted_values = sorted(values)

    def get_percentile(p: float) -> float:
        if len(sorted_values) == 1:
            return sorted_values[0]
        rank = (len(sorted_values) - 1) * p
        lower = int(rank)
        upper = min(lower + 1, len(sorted_values) - 1)
        weight = rank - lower
        return sorted_values[lower] * (1.0 - weight) + sorted_values[upper] * weight

    return {
        "avg_ms": statistics.mean(values),
        "p50_ms": get_percentile(0.50),
        "p95_ms": get_percentile(0.95),
        "p99_ms": get_percentile(0.99),
    }


def snapshot_thread_stats(benchmark: StorageBenchmark) -> Dict[str, Any]:
    storage = benchmark.storage
    total_pages = benchmark.stats["read_pages"] + benchmark.stats["write_pages"]
    return {
        "total_requests": benchmark.stats["total_requests"],
        "total_tokens": benchmark.stats["total_tokens"],
        "read_pages": benchmark.stats["read_pages"],
        "write_pages": benchmark.stats["write_pages"],
        "page_hits": benchmark.stats["page_hits"],
        "request_io_latencies_ms": list(benchmark.stats["request_io_latencies_ms"]),
        "request_wall_latencies_ms": list(benchmark.stats["request_wall_latencies_ms"]),
        "read_bytes": storage.stats["read_bytes"],
        "write_bytes": storage.stats["write_bytes"],
        "read_time_s": storage.stats["read_time_s"],
        "write_time_s": storage.stats["write_time_s"],
        "read_latencies_ms": list(storage.stats["read_latencies_ms"]),
        "write_latencies_ms": list(storage.stats["write_latencies_ms"]),
        "sync_count": storage.stats["sync_count"],
        "max_pages": storage.max_pages,
        "written_pages": len(storage._written_pages),
        "total_pages": total_pages,
    }


def aggregate_thread_stats(thread_stats: List[Dict[str, Any]]) -> Dict:
    total_requests = sum(s["total_requests"] for s in thread_stats)
    total_tokens = sum(s["total_tokens"] for s in thread_stats)
    read_pages = sum(s["read_pages"] for s in thread_stats)
    write_pages = sum(s["write_pages"] for s in thread_stats)
    page_hits = sum(s["page_hits"] for s in thread_stats)
    total_pages = read_pages + write_pages

    request_io_latencies = []
    request_wall_latencies = []
    read_latencies = []
    write_latencies = []
    for stats in thread_stats:
        request_io_latencies.extend(stats["request_io_latencies_ms"])
        request_wall_latencies.extend(stats["request_wall_latencies_ms"])
        read_latencies.extend(stats["read_latencies_ms"])
        write_latencies.extend(stats["write_latencies_ms"])

    read_bytes = sum(s["read_bytes"] for s in thread_stats)
    write_bytes = sum(s["write_bytes"] for s in thread_stats)
    read_time = sum(s["read_time_s"] for s in thread_stats)
    write_time = sum(s["write_time_s"] for s in thread_stats)

    return {
        "total_requests": total_requests,
        "total_tokens": total_tokens,
        "total_pages": total_pages,
        "read_pages": read_pages,
        "write_pages": write_pages,
        "page_hits": page_hits,
        "page_hit_rate": read_pages / total_pages if total_pages > 0 else 0,
        "write_ratio": write_pages / total_pages if total_pages > 0 else 0,
        "request_io_latency": latency_stats(request_io_latencies),
        "request_wall_latency": latency_stats(request_wall_latencies),
        "storage": {
            "read": {
                "count": read_pages,
                "mb": read_bytes / 1024 / 1024,
                "time_s": read_time,
                **latency_stats(read_latencies),
            },
            "write": {
                "count": write_pages,
                "mb": write_bytes / 1024 / 1024,
                "time_s": write_time,
                **latency_stats(write_latencies),
            },
            "sync_count": sum(s["sync_count"] for s in thread_stats),
            "max_pages": sum(s["max_pages"] for s in thread_stats),
            "written_pages": sum(s["written_pages"] for s in thread_stats),
            "page_hits": page_hits,
            "page_misses": write_pages,
        },
    }


def print_progress(
    done: int,
    total: int,
    start_time: float,
    stats: Dict,
    req: KVCacheRequest = None,
    suffix: str = "",
):
    elapsed = time.perf_counter() - start_time
    qps = done / elapsed if elapsed > 0 else 0
    storage = stats.get("storage", {})
    read_stats = storage.get("read", {})
    write_stats = storage.get("write", {})
    read_time = read_stats.get("time_s", 0)
    write_time = write_stats.get("time_s", 0)
    read_mbps = read_stats.get("mb", 0) / read_time if read_time > 0 else 0
    write_mbps = write_stats.get("mb", 0) / write_time if write_time > 0 else 0

    if req is None:
        req_info = ""
    else:
        req_info = (
            f" ids={len(req.hash_ids):3d} "
            f"tokens={req.input_length + req.output_length:6d} |"
        )

    print(
        f"  [{done:5d}/{total}]{req_info} QPS={qps:7.2f} | "
        f"R={stats['read_pages']:6d} "
        f"({read_stats.get('avg_ms', 0):6.2f}ms, {read_mbps:6.1f}MB/s) | "
        f"W={stats['write_pages']:6d} "
        f"({write_stats.get('avg_ms', 0):6.2f}ms, {write_mbps:6.1f}MB/s)"
        f"{suffix}"
    )


def should_print_progress(done: int, total: int, progress_interval: int) -> bool:
    if done >= total:
        return True
    return progress_interval > 0 and done % progress_interval == 0


def run_single_thread(
    benchmark: StorageBenchmark,
    requests: List[KVCacheRequest],
    replay_scale: float,
    progress_interval: int,
) -> Dict[str, Any]:
    start_time = time.perf_counter()
    base_timestamp = requests[0].timestamp if requests else 0
    completed = 0

    for req in requests:
        wait_for_replay_time(req, base_timestamp, start_time, replay_scale)
        benchmark.process_request(req)
        completed += 1
        if should_print_progress(completed, len(requests), progress_interval):
            print_progress(
                completed, len(requests), start_time, benchmark.get_stats(), req
            )

    return {
        "completed": completed,
        "elapsed": time.perf_counter() - start_time,
        "stats": benchmark.get_stats(),
    }


def run_multi_thread(
    benchmarks: List[StorageBenchmark],
    requests: List[KVCacheRequest],
    replay_scale: float,
) -> Dict[str, Any]:
    start_time = time.perf_counter()
    base_timestamp = requests[0].timestamp if requests else 0
    total_requests = len(requests) * len(benchmarks)
    completed = 0

    def run_worker(thread_id: int):
        benchmark = benchmarks[thread_id]
        for req in requests:
            wait_for_replay_time(req, base_timestamp, start_time, replay_scale)
            benchmark.process_request(req)
        return snapshot_thread_stats(benchmark)

    thread_stats = []
    with ThreadPoolExecutor(max_workers=len(benchmarks)) as executor:
        futures = [
            executor.submit(run_worker, thread_id)
            for thread_id in range(len(benchmarks))
        ]
        for future in as_completed(futures):
            worker_stats = future.result()
            thread_stats.append(worker_stats)
            completed += worker_stats["total_requests"]
            print_progress(
                completed,
                total_requests,
                start_time,
                aggregate_thread_stats(thread_stats),
                suffix=" | completed worker",
            )

    return {
        "completed": completed,
        "elapsed": time.perf_counter() - start_time,
        "stats": aggregate_thread_stats(thread_stats),
    }


def run_benchmark(
    trace_path: str,
    model_config: dict,
    backend: str = "disk",
    storage_dir: str = "/tmp/certus_mooncake_bench",
    max_requests: int = None,
    max_pages: int = None,
    page_size_tokens: int = 512,
    file_mode: str = "single",
    fsync_mode: str = "none",
    fsync_batch_size: int = 100,
    threads: int = 1,
    replay_scale: float = 0.0,
    progress_interval: int = 100,
    shm_path: str = "/dev/shm/certus-shmq",
    gpu_device: int = 0,
) -> Dict:
    """Run benchmark

    Args:
        trace_path: Trace file path
        model_config: Model configuration
        backend: 'disk' or 'certus'
        storage_dir: Storage directory (disk backend)
        max_requests: Maximum number of requests (None = all)
        max_pages: Maximum number of pages (None = auto-calculate)
        page_size_tokens: Tokens per page
        file_mode: 'single' or 'per-file' (disk backend)
        fsync_mode: When to fsync (disk backend)
        fsync_batch_size: Writes between fsync (disk backend)
        threads: Benchmark client worker threads
        replay_scale: Timestamp replay multiplier; 0 runs unpaced
        progress_interval: Print progress every N requests; 0 disables
        shm_path: certus-server shmq mailbox path (certus backend)
        gpu_device: CUDA device for GPU buffers (certus backend)

    Returns:
        Benchmark results dictionary
    """
    print(f"\n{'='*80}")
    print(f"Running: {Path(trace_path).name}")
    print(f"Backend: {backend}")
    print(f"Model: {model_config['name']}")
    if 'num_layers' in model_config:
        print(f"Layers: {model_config['num_layers']}")
    else:
        print(f"Bytes/token: {model_config['bytes_per_token']:,}")
    print(f"Page size: {page_size_tokens} tokens")
    if backend == "disk":
        print(f"File mode: {file_mode}")
    print(f"Threads: {threads}")
    print(
        f"Fast-forward: {replay_scale:g}x"
        if replay_scale > 0
        else "Fast-forward: unpaced"
    )
    print(f"{'='*80}")

    # Load trace
    replay = TraceReplay(trace_path)
    requests = replay.load_all()

    if max_requests:
        requests = requests[:max_requests]

    print(f"Loaded {len(requests)} requests")

    # Find max page_id from trace
    max_page_id = get_max_page_id(requests)
    max_pages_needed = max_page_id + 1  # page_id is 0-based

    # Create layout to get page size
    layout = create_layout(model_config, page_size_tokens)
    page_size_bytes = layout.value_size_bytes

    # Determine max_pages
    if max_pages is None:
        max_pages = max_pages_needed
        max_pages = ((max_pages + 999) // 1000) * 1000
    else:
        pass

    max_size_gb = max_pages * page_size_bytes / (1024**3)
    trace_size_gb = max_pages_needed * page_size_bytes / (1024**3)

    print("\n[Storage Configuration]")
    print(f"  Backend:                          {backend}")
    print(f"  Max page_id in trace:             {max_page_id:,}")
    print(f"  Pages needed (trace):             {max_pages_needed:,}")
    print(f"  Trace storage size:               {trace_size_gb:.2f} GB")
    print(f"  Max pages configured:             {max_pages:,}")
    if threads > 1:
        print(f"  Max pages across threads:         {max_pages * threads:,}")
    print(f"  Max storage available:            {max_size_gb:.2f} GB")
    if threads > 1:
        print(f"  Max storage across threads:       {max_size_gb * threads:.2f} GB")
    if backend == "certus":
        print(f"  shmq path:                        {shm_path}")
        print(f"  GPU device:                       {gpu_device}")

    if max_pages_needed > max_pages:
        shortfall = max_pages_needed - max_pages
        shortfall_gb = shortfall * page_size_bytes / (1024**3)
        print(
            f"\n  ⚠️  Storage insufficient: {shortfall:,} pages shortfall ({shortfall_gb:.2f} GB)"
        )
        print(
            f"  ⚠️  Consider increasing --max-pages to at least {max_pages_needed:,} for full simulation"
        )
    else:
        surplus = max_pages - max_pages_needed
        print(
            f"  ✓ Direct mapping: all {max_pages_needed:,} logical pages uniquely mapped"
        )

    storage_kwargs = {
        "page_size": page_size_bytes,
        "max_pages": max_pages,
    }
    if backend == "disk":
        storage_kwargs.update({
            "storage_dir": storage_dir,
            "file_mode": file_mode,
            "fsync_mode": fsync_mode,
            "fsync_batch_size": fsync_batch_size,
        })
    elif backend == "certus":
        storage_kwargs.update({
            "shm_path": shm_path,
            "gpu_device": gpu_device,
        })

    try:
        if threads <= 1:
            storage = create_storage(backend, **storage_kwargs)
            with StorageBenchmark(
                storage=storage,
                model_config=model_config,
                page_size_tokens=page_size_tokens,
            ) as benchmark:
                result = run_single_thread(
                    benchmark, requests, replay_scale, progress_interval
                )
        else:
            with ExitStack() as stack:
                benchmarks = []
                for thread_id in range(threads):
                    if backend == "disk":
                        kw = dict(storage_kwargs)
                        kw["storage_dir"] = str(
                            Path(storage_dir) / f"thread_{thread_id}"
                        )
                    else:
                        kw = dict(storage_kwargs)
                    s = create_storage(backend, **kw)
                    bench = stack.enter_context(
                        StorageBenchmark(
                            storage=s,
                            model_config=model_config,
                            page_size_tokens=page_size_tokens,
                        )
                    )
                    benchmarks.append(bench)
                result = run_multi_thread(benchmarks, requests, replay_scale)
    except KeyboardInterrupt:
        print(f"\n\n{'='*80}")
        print("Interrupted! Showing partial results:")
        print(f"{'='*80}")
        result = (
            result
            if "result" in locals()
            else {
                "completed": 0,
                "elapsed": 0,
                "stats": {},
            }
        )
        print_results(
            [
                {
                    "trace_file": Path(trace_path).name,
                    "total_requests": result["completed"],
                    "io_time_s": result["elapsed"],
                    "requests_per_second": (
                        result["completed"] / result["elapsed"]
                        if result["elapsed"] > 0
                        else 0
                    ),
                    "model": model_config["name"],
                    "backend": backend,
                    "fsync_mode": fsync_mode,
                    "threads": threads,
                    "replay_scale": replay_scale,
                    **result["stats"],
                }
            ]
        )
        sys.exit(0)

    return {
        "trace_file": Path(trace_path).name,
        "total_requests": result["completed"],
        "io_time_s": result["elapsed"],
        "requests_per_second": (
            result["completed"] / result["elapsed"] if result["elapsed"] > 0 else 0
        ),
        "model": model_config["name"],
        "backend": backend,
        "fsync_mode": fsync_mode,
        "threads": threads,
        "replay_scale": replay_scale,
        **result["stats"],
    }


# ============================================================================
# Output Formatting
# ============================================================================


def format_storage_stats(stats: Dict, title: str = "Storage"):
    """Format storage statistics with clear read/write separation"""
    storage = stats.get("storage", {})
    read_stats = storage.get("read", {})
    write_stats = storage.get("write", {})
    request_wall = stats.get("request_wall_latency", {})
    request_io = stats.get("request_io_latency", {})

    output = []
    output.append(f"\n[{title}]")

    # General info
    output.append("\n[General]")
    output.append(f"  Model:            {stats.get('model', 'N/A')}")
    output.append(f"  Backend:          {stats.get('backend', 'disk')}")
    output.append(f"  Threads:          {stats.get('threads', 1)}")
    replay_scale = stats.get("replay_scale", 0)
    output.append(
        f"  Fast-forward:     {f'{replay_scale:g}x' if replay_scale else 'unpaced'}"
    )
    output.append(f"  Requests:         {stats.get('total_requests', 0):,}")
    output.append(f"  Tokens:           {stats.get('total_tokens', 0):,}")
    output.append(f"  Total I/O Time:    {stats.get('io_time_s', 0):.3f} s")
    output.append(f"  QPS:              {stats.get('requests_per_second', 0):.2f}")
    output.append(f"  Hit Rate:         {stats.get('page_hit_rate', 0):.2%}")

    # Request Stats
    output.append("\n[Request Wall Latency]")
    output.append(f"  Avg:              {request_wall.get('avg_ms', 0):.3f} ms")
    output.append(f"  P50:              {request_wall.get('p50_ms', 0):.3f} ms")
    output.append(f"  P95:              {request_wall.get('p95_ms', 0):.3f} ms")
    output.append(f"  P99:              {request_wall.get('p99_ms', 0):.3f} ms")

    output.append("\n[Request Storage I/O Latency]")
    output.append(f"  Avg:              {request_io.get('avg_ms', 0):.3f} ms")
    output.append(f"  P50:              {request_io.get('p50_ms', 0):.3f} ms")
    output.append(f"  P95:              {request_io.get('p95_ms', 0):.3f} ms")
    output.append(f"  P99:              {request_io.get('p99_ms', 0):.3f} ms")

    # Read Stats
    output.append("\n[Read Operations]")
    output.append(f"  Count:            {read_stats.get('count', 0):,}")
    output.append(f"  Data Volume:      {read_stats.get('mb', 0):.2f} MB")
    read_time = read_stats.get("time_s", 0)
    read_mbps = read_stats.get("mb", 0) / read_time if read_time > 0 else 0
    output.append(f"  Total Time:       {read_time:.3f} s")
    output.append(f"  Bandwidth:        {read_mbps:.2f} MB/s")
    output.append("  Latency:")
    output.append(f"    Avg:            {read_stats.get('avg_ms', 0):.3f} ms")
    output.append(f"    P50:            {read_stats.get('p50_ms', 0):.3f} ms")
    output.append(f"    P95:            {read_stats.get('p95_ms', 0):.3f} ms")
    output.append(f"    P99:            {read_stats.get('p99_ms', 0):.3f} ms")

    # Write Stats
    output.append("\n[Write Operations]")
    output.append(f"  Count:            {write_stats.get('count', 0):,}")
    output.append(f"  Data Volume:      {write_stats.get('mb', 0):.2f} MB")
    write_time = write_stats.get("time_s", 0)
    write_mbps = write_stats.get("mb", 0) / write_time if write_time > 0 else 0
    output.append(f"  Total Time:       {write_time:.3f} s")
    output.append(f"  Bandwidth:        {write_mbps:.2f} MB/s")
    output.append("  Latency:")
    output.append(f"    Avg:            {write_stats.get('avg_ms', 0):.3f} ms")
    output.append(f"    P50:            {write_stats.get('p50_ms', 0):.3f} ms")
    output.append(f"    P95:            {write_stats.get('p95_ms', 0):.3f} ms")
    output.append(f"    P99:            {write_stats.get('p99_ms', 0):.3f} ms")

    # Storage Info
    output.append("\n[Storage Info]")
    output.append(f"  Max Pages:        {storage.get('max_pages', 0):,}")
    output.append(f"  Written Pages:    {storage.get('written_pages', 0):,}")
    output.append(f"  Sync Count:       {storage.get('sync_count', 0):,}")

    return "\n".join(output)


def print_results(results: List[Dict]):
    """Print benchmark results"""
    for i, r in enumerate(results, 1):
        print(f"\n{'='*80}")
        print(f"  [{i}/{len(results)}] {r['trace_file']} ({r.get('backend', 'disk')})")
        print(f"{'='*80}")
        print(format_storage_stats(r))


# ============================================================================
# CLI Entry Point
# ============================================================================


def main():
    """Main entry point"""
    parser = argparse.ArgumentParser(
        description="Certus KVCache Storage Benchmark (Mooncake FAST25 trace replay)",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )

    parser.add_argument(
        "--backend",
        type=str,
        choices=["disk", "certus"],
        default="disk",
        help="Storage backend: disk (posix pread/pwrite) or certus (shmq)",
    )
    parser.add_argument(
        "--trace-dir",
        type=str,
        default=None,
        help="Trace files directory (default: mooncake_traces/ next to this script)",
    )
    parser.add_argument(
        "--scenario",
        type=str,
        choices=["conversation", "synthetic", "toolagent", "all"],
        default="toolagent",
        help="Test scenario",
    )
    parser.add_argument(
        "--storage-dir",
        type=str,
        default="/tmp/certus_mooncake_bench",
        help="Storage directory (disk backend)",
    )
    parser.add_argument(
        "--model",
        type=str,
        default="glm5",
        help="Model preset (glm5, kimi-k2.6, llama-3-8b, llama-3-70b, deepseek-v3, "
             "etc.). Use --list-models to see all available.",
    )
    parser.add_argument(
        "--bytes-per-token",
        type=int,
        default=None,
        help="Override KV cache bytes per token (creates a custom generic model, "
             "overrides --model)",
    )
    parser.add_argument(
        "--list-models",
        action="store_true",
        help="List all available model presets and exit",
    )
    parser.add_argument(
        "--page-size-tokens",
        type=int,
        default=512,
        help="Page size in tokens (default: 512)",
    )
    parser.add_argument(
        "--file-mode",
        type=str,
        choices=["single", "per-file"],
        default="single",
        help="Storage layout (disk backend): single = one data.bin; per-file = one file per page",
    )
    parser.add_argument(
        "--max-requests", type=int, default=None, help="Maximum number of requests"
    )
    parser.add_argument(
        "--max-pages", type=int, default=2000, help="Maximum number of pages"
    )
    parser.add_argument(
        "--fsync-mode",
        type=str,
        choices=["batch", "always", "end", "none"],
        default="none",
        help="When to fsync (disk backend)",
    )
    parser.add_argument(
        "--fsync-batch-size",
        type=int,
        default=100,
        help="Number of writes between fsync (disk backend)",
    )
    parser.add_argument(
        "--threads",
        type=int,
        default=1,
        help="Number of benchmark client worker threads",
    )
    parser.add_argument(
        "--replay-scales",
        type=str,
        default="0",
        help="Comma-separated trace fast-forward speeds; 0 means unpaced",
    )
    parser.add_argument(
        "--progress-interval",
        type=int,
        default=100,
        help="Print progress every N requests; 0 disables per-request progress",
    )

    # Certus-specific arguments
    parser.add_argument(
        "--shm-path",
        type=str,
        default="/dev/shm/certus-shmq",
        help="certus-server shmq mailbox path (certus backend)",
    )
    parser.add_argument(
        "--gpu-device",
        type=int,
        default=0,
        help="CUDA device for GPU buffers (certus backend)",
    )
    parser.add_argument(
        "--output-json",
        type=str,
        default=None,
        help="Write results to a JSON file (appends to existing array if file exists)",
    )
    parser.add_argument(
        "--tag",
        type=str,
        action="append",
        default=[],
        help="Key=value tags to embed in JSON results (repeatable, e.g. --tag drives=4)",
    )

    args = parser.parse_args()

    # --list-models: print all available models and exit
    if args.list_models:
        from layout import ALL_MODELS, MLA_MODEL_CONFIG, GENERIC_MODEL_CONFIG, create_layout
        print(f"\n{'Available Model Presets':^70}")
        print(f"{'='*70}")
        print(f"\n{'MLA Models (per-layer decomposition)':}")
        print(f"{'─'*70}")
        for name, cfg in MLA_MODEL_CONFIG.items():
            layout = create_layout(cfg, args.page_size_tokens)
            page_mb = layout.value_size_bytes / 1024 / 1024
            print(f"  {name:<20s} {cfg['num_layers']:>3d} layers  "
                  f"page={page_mb:>7.1f} MiB @ {args.page_size_tokens} tok/page")
        print(f"\n{'Generic Models (bytes_per_token)':}")
        print(f"{'─'*70}")
        for name, cfg in GENERIC_MODEL_CONFIG.items():
            bpt = cfg['bytes_per_token']
            page_bytes = bpt * args.page_size_tokens
            if page_bytes >= 1024 * 1024:
                page_str = f"{page_bytes / 1024 / 1024:>7.1f} MiB"
            elif page_bytes >= 1024:
                page_str = f"{page_bytes / 1024:>7.1f} KiB"
            else:
                page_str = f"{page_bytes:>7d} B  "
            notes = cfg.get('notes', '')
            print(f"  {name:<20s} {bpt:>10,d} B/tok  "
                  f"page={page_str} @ {args.page_size_tokens} tok/page"
                  f"{'  (' + notes + ')' if notes else ''}")
        print(f"\nUse --model=<name> or --bytes-per-token=N for custom.")
        sys.exit(0)

    if args.threads < 1:
        parser.error("--threads must be at least 1")
    if args.progress_interval < 0:
        parser.error("--progress-interval must be non-negative")

    # Default trace-dir to mooncake_traces/ next to this script
    if args.trace_dir is None:
        args.trace_dir = str(Path(__file__).resolve().parent / "mooncake_traces")

    # Build model config: --bytes-per-token overrides --model
    if args.bytes_per_token is not None:
        model_config = {
            "name": f"custom-{args.bytes_per_token}B",
            "bytes_per_token": args.bytes_per_token,
            "notes": "custom",
        }
    else:
        model_config = get_model_config(args.model)

    print(f"\n{'='*80}")
    print(f"{'Certus KVCache Storage Benchmark':^80}")
    print(f"{'(Mooncake FAST25 Trace Replay)':^80}")
    print(f"{'='*80}")

    # Print model info
    if 'num_layers' in model_config:
        print(f"Model: {model_config['name']} ({model_config['num_layers']} layers)")
    else:
        bpt = model_config['bytes_per_token']
        print(f"Model: {model_config['name']} ({bpt:,} bytes/token)")
    print(f"Backend: {args.backend}")
    replay_scales = parse_csv_floats(args.replay_scales)
    if not replay_scales:
        parser.error("--replay-scales must include at least one value")
    if any(scale < 0 for scale in replay_scales):
        parser.error("--replay-scales values must be non-negative")

    # Determine scenarios
    scenarios = (
        ["conversation", "synthetic", "toolagent"]
        if args.scenario == "all"
        else [args.scenario]
    )
    trace_files = {
        "conversation": "conversation_trace.jsonl",
        "synthetic": "synthetic_trace.jsonl",
        "toolagent": "toolagent_trace.jsonl",
    }

    # Run benchmarks
    results = []
    use_scale_subdirs = len(replay_scales) > 1 or replay_scales[0] != 0
    for scenario in scenarios:
        trace_path = Path(args.trace_dir) / trace_files[scenario]
        if trace_path.exists():
            for replay_scale in replay_scales:
                run_dir = Path(args.storage_dir) / scenario
                if use_scale_subdirs:
                    run_dir = run_dir / f"replay_{replay_scale:g}x"
                result = run_benchmark(
                    str(trace_path),
                    model_config,
                    backend=args.backend,
                    storage_dir=str(run_dir),
                    max_requests=args.max_requests,
                    max_pages=args.max_pages,
                    page_size_tokens=args.page_size_tokens,
                    file_mode=args.file_mode,
                    fsync_mode=args.fsync_mode,
                    fsync_batch_size=args.fsync_batch_size,
                    threads=args.threads,
                    replay_scale=replay_scale,
                    progress_interval=args.progress_interval,
                    shm_path=args.shm_path,
                    gpu_device=args.gpu_device,
                )
                results.append(result)
        else:
            print(f"Warning: Trace file not found: {trace_path}")

    # Embed --tag key=value pairs into each result
    if args.tag:
        tags = {}
        for t in args.tag:
            if "=" in t:
                k, v = t.split("=", 1)
                # Try to parse as int/float for cleaner JSON
                try:
                    v = int(v)
                except ValueError:
                    try:
                        v = float(v)
                    except ValueError:
                        pass
                tags[k] = v
        for r in results:
            r.update(tags)

    # Print results
    if results:
        print_results(results)
    else:
        print("Error: No trace files were successfully processed.", file=sys.stderr)
        sys.exit(1)

    # Write JSON output (append to existing array if file exists)
    if args.output_json and results:
        out_path = Path(args.output_json)
        existing = []
        if out_path.exists():
            try:
                existing = json.loads(out_path.read_text(encoding="utf-8"))
                if not isinstance(existing, list):
                    existing = [existing]
            except (json.JSONDecodeError, OSError):
                existing = []
        existing.extend(results)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(existing, indent=2), encoding="utf-8")
        print(f"\n[Results saved to {out_path} ({len(existing)} total entries)]")


if __name__ == "__main__":
    main()

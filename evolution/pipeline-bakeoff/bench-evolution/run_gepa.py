#!/usr/bin/env python3
"""Launch GEPA optimization of the Certus benchmark script.

Usage:
    cd evolution/pipeline-bakeoff/bench-evolution
    uv run --project /home/nara/certus/evo_frameworks/gepa python run_gepa.py

Requires: Certus server running on localhost:50051 (or set CERTUS_SERVER env var).
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from evaluate import evaluate

from gepa.optimize_anything import (
    EngineConfig,
    GEPAConfig,
    ReflectionConfig,
    optimize_anything,
    make_litellm_lm,
)

SEED_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "seed_program.py")

OBJECTIVE = (
    "Maximize aggregate cold-lookup throughput (GB/s) for 8 concurrent clients "
    "against the Certus gRPC dispatcher. The score is the 'aggregate=X.XX GB/s' "
    "value printed in the Lookup (cold) statistics line."
)

BACKGROUND = """\
## Hardware
- Server: 7x NVMe Gen4 SSDs (6 data + 1 metadata), NVIDIA A30 GPU, PCIe Gen4 x16
- Theoretical ceiling: ~25-28 GB/s (PCIe x16 to GPU)
- Memory-tier pool: 256 MiB DRAM staging (64 x 4 MiB objects)

## What the benchmark measures
Cold lookups = objects that were evicted from DRAM to NVMe SSD. The server reads
from NVMe via SPDK, DMAs into a ring buffer, then transfers to GPU via CUDA IPC.
The client provides a pre-allocated GPU buffer handle; server writes directly to it.

## gRPC specifics
- Protocol: unary gRPC (BatchLookupRequest with N entries per call)
- Each entry carries a CUDA IPC handle (64 bytes) identifying the GPU buffer
- Server writes 4 MiB per object directly to GPU memory via the IPC handle
- Channel options that matter: max_send/receive_message_length, keepalive settings

## CUDA IPC constraints
- One cudaIpcGetMemHandle per allocation (not per-request)
- Handle is 64 bytes, process-scoped, valid for the allocation lifetime
- Multiple concurrent requests CAN share one IPC handle IF they target the same buffer
  (server writes are sequential per-handle)
- For parallel throughput: each client thread needs its own GPU tensor + IPC handle

## Python GIL and concurrency
- gRPC Python releases the GIL during I/O (network send/recv)
- threading.Thread works well for I/O-bound gRPC clients
- asyncio with grpc.aio could reduce thread overhead for many clients
- The actual bottleneck is server-side NVMe read + GPU DMA, not client-side

## What MUST NOT change (evaluator will reject if violated)
- CLI arguments: --server, --clients, --num-objects, --iterations, --block-size
- Output format: must print "aggregate=X.XX GB/s" in the stats output
- Protobuf messages: dispatcher_pb2.BatchLookupRequest, LookupEntry, etc.
- The script must exit 0 on success, non-zero on errors
- Must import and use: dispatcher_pb2, dispatcher_pb2_grpc, torch, grpc

## Optimization directions to explore
1. Request pipelining: overlap gRPC requests (send next while waiting for response)
2. Async gRPC (grpc.aio): reduce thread creation overhead, enable true concurrency
3. Buffer pool pre-allocation: allocate all GPU tensors upfront, reuse across iterations
4. Batch size tuning: current batch is all num_objects at once; try sub-batching
5. Channel multiplexing: multiple gRPC channels per client for HTTP/2 stream parallelism
6. Connection warmup: pre-establish channels before the timed section
7. Reduce per-iteration overhead: minimize object creation in the hot loop
8. Barrier-free measurement: measure per-client independently, reduce sync points
"""


def main():
    with open(SEED_PATH) as f:
        seed = f.read()

    LITELLM_API_BASE = os.environ.get(
        "LITELLM_API_BASE", "https://ete-litellm.ai-models.vpc-int.res.ibm.com"
    )
    API_KEY = os.environ.get("LITELLM_API_KEY", "")
    if not API_KEY:
        key_path = "/tmp/.bakeoff_key"
        if os.path.exists(key_path):
            with open(key_path) as kf:
                API_KEY = kf.read().strip()
        else:
            raise RuntimeError(
                "No LITELLM_API_KEY env var and /tmp/.bakeoff_key not found. "
                "Set LITELLM_API_KEY or create /tmp/.bakeoff_key with your proxy token."
            )
    MODEL = os.environ.get("GEPA_MODEL", "openai/aws/claude-opus-4-6")

    lm = make_litellm_lm(MODEL, api_base=LITELLM_API_BASE, api_key=API_KEY, max_tokens=16384)

    result = optimize_anything(
        seed_candidate=seed,
        evaluator=evaluate,
        objective=OBJECTIVE,
        background=BACKGROUND,
        config=GEPAConfig(
            engine=EngineConfig(
                run_dir="outputs/bench_evolution",
                max_metric_calls=50,
                capture_stdio=True,
                cache_evaluation=True,
            ),
            reflection=ReflectionConfig(
                reflection_lm=lm,
            ),
        ),
    )

    print(f"\n{'='*70}")
    print("GEPA Optimization Complete")
    print(f"{'='*70}")
    best_score = max(result.val_aggregate_scores) if result.val_aggregate_scores else 0.0
    print(f"Best score: {best_score:.2f} GB/s")
    print(f"Candidates explored: {len(result.candidates)}")
    print(f"\nBest candidate saved to: outputs/bench_evolution/")

    best_path = os.path.join("outputs", "bench_evolution", "best_benchmark.py")
    os.makedirs(os.path.dirname(best_path), exist_ok=True)
    with open(best_path, "w") as f:
        f.write(result.best_candidate)
    print(f"Best script written to: {best_path}")


if __name__ == "__main__":
    main()

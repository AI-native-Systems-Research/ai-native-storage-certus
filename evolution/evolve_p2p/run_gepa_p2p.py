#!/usr/bin/env python3
"""Native GEPA runner for the P2P evolution experiment.

Uses GEPA's optimize_anything() with multi-file seed (pipeline.rs + lib.rs + dma.rs).
Scoring: 60% throughput + 40% latency (workload-first).

Usage:
    uv run --project /home/nara/certus/evo_frameworks/gepa \
        python run_gepa_p2p.py --iterations 30 --run-dir results/gepa_native

Environment:
    CERTUS_DATA_PCI: NVMe PCI address(es), comma-separated (default: 0000:62:00.0)
    LITELLM_API_KEY or /tmp/.bakeoff_key: LLM API key
    LITELLM_API_BASE: LiteLLM proxy URL
    GEPA_MODEL: Model name (default: openai/aws/claude-opus-4-6)
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "evaluator"))

from evaluate_p2p import evaluate

from gepa.optimize_anything import (
    EngineConfig,
    GEPAConfig,
    ReflectionConfig,
    optimize_anything,
    make_litellm_lm,
)
from gepa.core.callbacks import GEPACallback

EXPERIMENT_DIR = Path(__file__).resolve().parent
INITIAL_PROGRAMS = EXPERIMENT_DIR / "initial_programs"

OBJECTIVE = (
    "Maximize the fitness score for the storage server's SSD-to-GPU cold-lookup "
    "data path. The score rewards higher throughput (GB/s) and lower tail latency (p99). "
    "Data integrity is a hard constraint — corruption scores -1. "
    "The current code scores ~0.20. Higher scores are achievable."
)

_bg_file = os.environ.get("GEPA_BACKGROUND_FILE")
if _bg_file and os.path.isfile(_bg_file):
    BACKGROUND = open(_bg_file).read()
else:
    BACKGROUND = """\
## System Under Optimization

A storage server that moves data from NVMe SSDs to GPU memory for inference workloads.

## Hardware

- NVMe Gen4 SSDs via SPDK (userspace driver, no kernel filesystem)
- NVIDIA A30 GPU, PCIe Gen4 x16
- Kernel modules loaded: nvidia-peermem, gdrdrv
- 4096 hugepages allocated, memlock unlimited, NVMe devices bound to VFIO

## Fitness Function

score = 0.60 * throughput + 0.40 * latency

Where:
- throughput = min(1.0, cold_lookup_gbps / 12.0)
- latency = min(1.0, 0.4 / p99_latency_ms)

Hard constraints: build must succeed, data integrity must pass.

## Build & Evaluation

- cargo build -p certus-server --release --features p2p
- The p2p feature enables GPU-direct DMA buffer functions in gpu-services
- Evaluator starts the server and runs certus-api-bench.py automatically
- Changes must compile and pass data integrity checks

## Codebase

The gpu-services crate (components/gpu-services/src/) contains DMA buffer creation
functions for various memory types. The dispatcher crate (components/dispatcher/src/)
manages the transfer pipeline. The interfaces crate defines DmaBuffer, GpuStream, etc.
"""


class ScoresCallback(GEPACallback):
    """Write per-evaluation scores to jsonl."""

    def __init__(self, scores_path: Path):
        self.scores_path = scores_path
        self.scores_path.parent.mkdir(parents=True, exist_ok=True)
        self._eval_count = 0

    def on_evaluation_end(self, event) -> None:
        """Fires after each evaluator call with full results."""
        iteration = event.get("iteration", 0)
        scores = event.get("scores", [])
        outputs = event.get("outputs", [])

        for i, score in enumerate(scores):
            self._eval_count += 1
            metrics = {}
            error = ""

            # outputs[i] is (score, output_text, side_info_dict) from GEPA's internal handling
            if i < len(outputs):
                out = outputs[i]
                if isinstance(out, dict):
                    metrics = out
                elif isinstance(out, tuple) and len(out) >= 3:
                    metrics = out[2] if isinstance(out[2], dict) else {}
                elif isinstance(out, tuple) and len(out) >= 2:
                    metrics = out[1] if isinstance(out[1], dict) else {}

            build_ok = metrics.get("build_succeeded", score > 0)
            data_ok = metrics.get("data_integrity", True)
            error = metrics.get("error", "")
            if not error and not build_ok:
                error = metrics.get("log", "")

            if not build_ok:
                failure_type = "build_failure"
            elif "Server" in error or "startup" in str(error).lower():
                failure_type = "server_startup_failure"
            elif "timeout" in str(error).lower():
                failure_type = "benchmark_timeout"
            elif not data_ok:
                failure_type = "integrity_failure"
            elif score > 0:
                failure_type = "success"
            else:
                failure_type = "other_failure"

            entry = {
                "combined_score": round(score, 4) if score else 0.0,
                "iteration": self._eval_count,
                "gepa_iteration": iteration,
                "throughput_gbps": metrics.get("throughput_gbps"),
                "p99_latency_ms": metrics.get("p99_latency_ms"),
                "p50_latency_ms": metrics.get("p50_latency_ms"),
                "mean_latency_ms": metrics.get("mean_latency_ms"),
                "cpu_util_fraction": metrics.get("cpu_util_fraction"),
                "build_succeeded": build_ok,
                "data_integrity": data_ok,
                "failure_type": failure_type,
                "error": str(error)[:200] if error else None,
            }
            with open(self.scores_path, "a") as f:
                f.write(json.dumps(entry) + "\n")


def load_seed() -> dict[str, str]:
    """Load wild-type seed files as multi-file dict.

    Only pipeline.rs + dma.rs — lib.rs is too large (3244 lines) for
    full-rewrite mode. P2P can be implemented within these two files
    by modifying PipelineRing::new() to create GPU BAR buffers.
    """
    seed_files = {
        "pipeline.rs": INITIAL_PROGRAMS / "pipeline.rs",
        "dma.rs": INITIAL_PROGRAMS / "dma.rs",
    }
    seed = {}
    for name, path in seed_files.items():
        if path.exists():
            seed[name] = path.read_text()
    if not seed:
        raise FileNotFoundError(f"No seed files found in {INITIAL_PROGRAMS}")
    return seed


def get_api_credentials() -> tuple[str, str]:
    api_base = os.environ.get(
        "LITELLM_API_BASE", "https://ete-litellm.ai-models.vpc-int.res.ibm.com"
    )
    api_key = os.environ.get("LITELLM_API_KEY", "")
    if not api_key:
        key_path = "/tmp/.bakeoff_key"
        if os.path.exists(key_path):
            with open(key_path) as f:
                api_key = f.read().strip()
        else:
            raise RuntimeError(
                "No LITELLM_API_KEY env var and /tmp/.bakeoff_key not found."
            )
    return api_base, api_key


def main():
    parser = argparse.ArgumentParser(description="GEPA P2P evolution runner")
    parser.add_argument(
        "--iterations", type=int, default=30,
        help="Max evaluation calls (default: 30)",
    )
    parser.add_argument(
        "--run-dir", type=str, default=None,
        help="Output directory for results",
    )
    args = parser.parse_args()

    # Resolve run directory
    if args.run_dir:
        run_dir = Path(args.run_dir)
    else:
        run_dir = EXPERIMENT_DIR / "results" / "gepa_native"
    run_dir.mkdir(parents=True, exist_ok=True)

    # Load seed
    seed = load_seed()
    print(f"Seed files: {list(seed.keys())}")
    print(f"Run dir: {run_dir}")
    print(f"Max evaluations: {args.iterations}")

    # Setup LLM
    api_base, api_key = get_api_credentials()
    model = os.environ.get("GEPA_MODEL", "openai/aws/claude-opus-4-6")
    lm = make_litellm_lm(model, api_base=api_base, api_key=api_key, max_tokens=16384)

    scores_callback = ScoresCallback(run_dir / "scores.jsonl")

    result = optimize_anything(
        seed_candidate=seed,
        evaluator=evaluate,
        objective=OBJECTIVE,
        background=BACKGROUND,
        config=GEPAConfig(
            engine=EngineConfig(
                run_dir=str(run_dir),
                max_metric_calls=args.iterations,
                capture_stdio=True,
                cache_evaluation=True,
                parallel=False,
                display_progress_bar=True,
            ),
            reflection=ReflectionConfig(
                reflection_lm=lm,
                module_selector="all",
            ),
            callbacks=[scores_callback],
        ),
    )

    print(f"\n{'='*70}")
    print("GEPA P2P Evolution Complete")
    print(f"{'='*70}")
    best_score = max(result.val_aggregate_scores) if result.val_aggregate_scores else 0.0
    print(f"Best score: {best_score:.4f}")
    print(f"Candidates explored: {len(result.candidates)}")
    print(f"Results saved to: {run_dir}")

    # Save best candidate
    best_path = run_dir / "best"
    best_path.mkdir(exist_ok=True)
    if isinstance(result.best_candidate, dict):
        for name, content in result.best_candidate.items():
            (best_path / name).write_text(content)
    else:
        (best_path / "pipeline.rs").write_text(result.best_candidate)
    print(f"Best candidate: {best_path}")

    # Save all candidates (including failed) for dashboard source viewing
    candidates_path = run_dir / "candidates"
    candidates_path.mkdir(exist_ok=True)
    for idx, candidate in enumerate(result.candidates):
        gen_dir = candidates_path / f"gen_{idx}"
        gen_dir.mkdir(exist_ok=True)
        if isinstance(candidate, dict):
            parts = []
            for name, content in candidate.items():
                (gen_dir / name).write_text(content)
                parts.append(f"// --- FILE: {name} ---\n{content}")
            (gen_dir / "main.rs").write_text("\n\n".join(parts))
        elif isinstance(candidate, str):
            (gen_dir / "main.rs").write_text(candidate)
    print(f"Saved {len(result.candidates)} candidates to: {candidates_path}")


if __name__ == "__main__":
    main()

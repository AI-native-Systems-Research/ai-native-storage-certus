#!/usr/bin/env python3
"""Native GEPA runner for the P2P evolution experiment.

Supports two phases:
  - Phase 2 (knobs): Evolves pipeline_config.rs constants
  - Phase 3 (policy): Evolves transfer_policy.py decision function

Usage:
    uv run --project /home/nara/certus/evo_frameworks/gepa \
        python run_gepa_p2p.py --phase knobs --iterations 50

Environment:
    P2P_NVME_PCI: NVMe PCI address for benchmark (optional, auto-detect)
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

OBJECTIVES = {
    "knobs": (
        "Maximize the composite fitness score for the storage server's SSD-to-GPU "
        "cold-lookup data path. The score heavily rewards low CPU memory traffic and "
        "low CPU utilization during transfers (60% of total score). The current "
        "implementation scores ~0.55. Higher scores are achievable by reducing CPU "
        "involvement in the data movement."
    ),
    "policy": (
        "Maximize the composite fitness score by restructuring how data moves from "
        "NVMe SSD to GPU memory. The scoring function penalizes CPU memory bandwidth "
        "consumption and CPU utilization during transfers. Explore the available DMA "
        "buffer creation functions in the gpu-services crate and the hardware capabilities "
        "(nvidia-peermem, SPDK memory registration) to find a more efficient data path."
    ),
}

BACKGROUND = """\
## System Under Optimization

A storage server that moves data from NVMe SSDs to GPU memory for inference workloads.
Currently scored at ~0.55 on the fitness function. Higher scores are achievable.

## Hardware

- 7x NVMe Gen4 SSDs via SPDK (userspace driver, no kernel filesystem, 128 KiB MDTS)
- NVIDIA A30 GPU, PCIe Gen4 x16
- Kernel modules loaded: nvidia-peermem, gdrdrv
- 2048 hugepages allocated, memlock unlimited, NVMe devices bound to VFIO

## Fitness Function

Measures: throughput, tail latency, host CPU memory bandwidth during transfer,
CPU utilization during transfer, run-to-run stability, data correctness.

Higher throughput and lower latency are the primary objectives. Lower CPU memory
traffic and CPU utilization also contribute to the score. The current implementation
scores ~0.55. Higher scores are achievable.

## Build & Evaluation

- Must compile: cargo build -p certus-server --release
- Evaluator automatically starts the server and runs a benchmark client
- Data integrity is verified — corruption scores -1.0
- NVMe chunk size must not exceed MDTS (128 KiB = 131072 bytes)

## Codebase Reference

The gpu-services crate (components/gpu-services/src/) contains functions for creating
DMA buffers from various memory types. The dispatcher crate manages the transfer pipeline.
The interfaces crate defines DmaBuffer, GpuStream, and related types.
"""


class ScoresCallback(GEPACallback):
    """Write scores to jsonl for orchestrator integration."""

    def __init__(self, scores_path: Path):
        self.scores_path = scores_path
        self.scores_path.parent.mkdir(parents=True, exist_ok=True)

    def on_iteration_end(self, event: dict) -> None:
        score = event.get("val_score")
        if score is not None:
            with open(self.scores_path, "a") as f:
                f.write(json.dumps({"combined_score": round(score, 4)}) + "\n")


def load_seed(phase: str) -> str | dict[str, str]:
    """Load seed candidate based on phase."""
    if phase == "knobs":
        return (INITIAL_PROGRAMS / "pipeline_config.rs").read_text()
    elif phase == "policy":
        return (INITIAL_PROGRAMS / "transfer_policy.py").read_text()
    else:
        raise ValueError(f"Unknown phase: {phase}")


def get_api_credentials() -> tuple[str, str]:
    """Get LiteLLM API base and key."""
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
        "--phase", choices=["knobs", "policy"], default="knobs",
        help="Evolution phase (default: knobs)",
    )
    parser.add_argument(
        "--iterations", type=int, default=50,
        help="Max evaluation calls (default: 50)",
    )
    parser.add_argument(
        "--run-dir", type=str, default=None,
        help="Output directory for results",
    )
    parser.add_argument(
        "--profile", choices=["A", "B", "C"], default="B",
        help="Scoring profile: A=workload-first, B=balanced, C=architecture-pressure (default: B)",
    )
    args = parser.parse_args()

    # Set scoring profile for evaluator
    os.environ["P2P_SCORING_PROFILE"] = args.profile

    # Resolve run directory
    if args.run_dir:
        run_dir = Path(args.run_dir)
    else:
        run_dir = EXPERIMENT_DIR / "results" / f"gepa_{args.phase}"
    run_dir.mkdir(parents=True, exist_ok=True)

    # Load seed
    seed = load_seed(args.phase)
    print(f"Phase: {args.phase}")
    print(f"Run dir: {run_dir}")
    print(f"Max evaluations: {args.iterations}")
    print(f"Seed length: {len(seed)} chars")

    # Setup LLM
    api_base, api_key = get_api_credentials()
    model = os.environ.get("GEPA_MODEL", "openai/aws/claude-opus-4-6")
    lm = make_litellm_lm(model, api_base=api_base, api_key=api_key, max_tokens=16384)

    scores_callback = ScoresCallback(run_dir / "scores.jsonl")

    result = optimize_anything(
        seed_candidate=seed,
        evaluator=evaluate,
        objective=OBJECTIVES[args.phase],
        background=BACKGROUND,
        config=GEPAConfig(
            engine=EngineConfig(
                run_dir=str(run_dir),
                max_metric_calls=args.iterations,
                capture_stdio=True,
                cache_evaluation=True,
                parallel=False,  # benchmark uses GPU exclusively
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
    print(f"GEPA P2P Evolution Complete — Phase: {args.phase}")
    print(f"{'='*70}")
    best_score = max(result.val_aggregate_scores) if result.val_aggregate_scores else 0.0
    print(f"Best score: {best_score:.4f}")
    print(f"Candidates explored: {len(result.candidates)}")
    print(f"Results saved to: {run_dir}")

    # Save best candidate
    best_path = run_dir / f"best_{args.phase}"
    best_path.mkdir(exist_ok=True)
    ext = ".rs" if args.phase == "knobs" else ".py"
    if isinstance(result.best_candidate, dict):
        for name, content in result.best_candidate.items():
            (best_path / name).write_text(content)
    else:
        (best_path / f"best_candidate{ext}").write_text(result.best_candidate)
    print(f"Best candidate: {best_path}")


if __name__ == "__main__":
    main()

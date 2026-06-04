#!/usr/bin/env python3
"""Run native GEPA optimization for the pipeline bakeoff.

Supports all eval modes (fixed/mixed/concurrent) with proper multi-file
seed candidates. Uses GEPA's optimize_anything() directly instead of
routing through SkyDiscover.

Usage:
    uv run --project /home/nara/certus/evo_frameworks/gepa \
        python run_gepa_native.py --iterations 30 --eval-mode concurrent --run-dir results/gepa

Environment:
    BAKEOFF_EVAL_MODE: Set automatically from --eval-mode
    LITELLM_API_KEY or /tmp/.bakeoff_key: LLM API key
    LITELLM_API_BASE: LiteLLM proxy URL (default: IBM internal)
    GEPA_MODEL: Model name (default: openai/aws/claude-opus-4-6)
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "evaluator"))

from evaluate_gepa import evaluate

from gepa.optimize_anything import (
    EngineConfig,
    GEPAConfig,
    ReflectionConfig,
    optimize_anything,
    make_litellm_lm,
)
from gepa.core.callbacks import GEPACallback

BAKEOFF_DIR = Path(__file__).resolve().parent

HYPOTHESIS_DIRS = {
    "fixed": BAKEOFF_DIR / "hypothesis_1",
    "mixed": BAKEOFF_DIR / "hypothesis_2",
    "concurrent": BAKEOFF_DIR / "hypothesis_3",
}

OBJECTIVES = {
    "fixed": (
        "Maximize single-client cold-lookup throughput (GB/s) for 4 MiB objects "
        "from NVMe SSD through the Certus dispatcher pipeline to GPU memory."
    ),
    "mixed": (
        "Maximize composite cold-lookup throughput (GB/s) across mixed object sizes "
        "(1/2/4/16 MiB) from NVMe SSD through the Certus dispatcher pipeline to GPU."
    ),
    "concurrent": (
        "Maximize aggregate cold-lookup throughput (GB/s) with 8 concurrent clients "
        "against the Certus gRPC dispatcher. The bottleneck is Mutex serialization "
        "in the dispatcher — resolve contention to scale beyond single-client throughput."
    ),
}

BACKGROUND_COMMON = """\
## Hardware
- Server: 7x NVMe Gen4 SSDs (6 data + 1 metadata), NVIDIA A30 GPU, PCIe Gen4 x16
- Per-drive sequential read: ~3.5 GB/s
- Aggregate NVMe bandwidth (7 drives): ~24.5 GB/s theoretical
- GPU PCIe Gen4 x16 bandwidth: ~25 GB/s
- Each NVMe supports QD=32 at 128 KiB MDTS

## Data Path
Cold lookups: NVMe read via SPDK → DRAM ring buffer → CUDA DMA to GPU
The pipeline uses zero-copy depth (QD) to overlap NVMe reads with DMA transfers.

## Build & Evaluation
- Must compile: cargo build -p certus-server --release
- Must pass data integrity verification (correct data delivered to GPU)
- Evaluator: patches files into source tree, builds, restarts server, benchmarks
- Score = aggregate cold lookup throughput in GB/s

## Constraints
- IDispatcher trait signatures cannot change
- Must use interfaces crate types (Command, Completion, DmaBuffer, GpuStream)
- define_component! macro has limited flexibility
"""

BACKGROUND_CONCURRENT = """\
## Concurrency Bottleneck (H3)
The dispatcher holds:
  data_drives: Mutex<Vec<DataDrive>>          — serializes drive access
  pipeline_ring: Mutex<Option<PipelineRing>>  — serializes entire pipeline

With 8 clients: only 1 runs the pipeline at a time, others blocked on Mutex.
Effective throughput = single-client (~5 GB/s), NOT 8x.

## Architectural Opportunities
1. Remove outer Arc<Mutex<Arc<dyn IDispatcher>>> in service.rs (biggest win)
2. Drive-sharded pipeline rings (one per NVMe, keys already sharded by drive_index)
3. RwLock for read-only data_drives
4. Per-client or pooled CUDA streams
5. Lock-free patterns (crossbeam ArrayQueue for ring pool)

## Files in Scope
- service.rs: gRPC handler with outermost Mutex (THE #1 bottleneck)
- lib.rs: Dispatcher component with Mutex<> fields
- pipeline.rs: NVMe→DRAM→GPU transfer logic (constants already optimized from H1)

## Targets
- Baseline: ~5 GB/s aggregate (mutex-bound)
- Realistic target: 15-20 GB/s
- Theoretical ceiling: ~25 GB/s (PCIe x16)
"""


class ScoresCallback(GEPACallback):
    """Write scores to jsonl as evaluations complete."""

    def __init__(self, scores_path: Path):
        self.scores_path = scores_path
        self.scores_path.parent.mkdir(parents=True, exist_ok=True)

    def on_iteration_end(self, event: dict) -> None:
        score = event.get("val_score")
        if score is not None and score > 0:
            with open(self.scores_path, "a") as f:
                f.write(json.dumps({"combined_score": round(score, 4)}) + "\n")


def load_seed_candidate(eval_mode: str) -> dict[str, str]:
    """Load seed candidate files based on evaluation mode."""
    h_dir = HYPOTHESIS_DIRS[eval_mode]

    if eval_mode == "concurrent":
        init_dir = h_dir / "initial_program_dir"
        if init_dir.exists():
            candidate = {}
            for f in init_dir.iterdir():
                if f.suffix == ".rs":
                    candidate[f.name] = f.read_text()
            if candidate:
                return candidate

    # Single-file fallback: pipeline.rs only
    init_file = h_dir / "initial_program.rs"
    if init_file.exists():
        content = init_file.read_text()
        # If it's a concatenated H3 file, extract the pipeline.rs section
        if "// === FILE:" in content and eval_mode != "concurrent":
            # For non-concurrent modes using concatenated files, just use pipeline.rs section
            return {"pipeline.rs": content}
        return {"pipeline.rs": content}

    raise FileNotFoundError(f"No initial program found in {h_dir}")


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
                "No LITELLM_API_KEY env var and /tmp/.bakeoff_key not found. "
                "Set LITELLM_API_KEY or create /tmp/.bakeoff_key with your proxy token."
            )
    return api_base, api_key


def main():
    parser = argparse.ArgumentParser(description="Native GEPA runner for pipeline bakeoff")
    parser.add_argument(
        "--iterations", type=int, default=30,
        help="Max evaluation calls (default: 30)",
    )
    parser.add_argument(
        "--eval-mode", choices=["fixed", "mixed", "concurrent"], default="fixed",
        help="Evaluation mode (default: fixed)",
    )
    parser.add_argument(
        "--run-dir", type=str, default=None,
        help="Output directory for GEPA state and results",
    )
    args = parser.parse_args()

    # Set eval mode env var for the evaluator
    os.environ["BAKEOFF_EVAL_MODE"] = args.eval_mode

    # Resolve run directory
    if args.run_dir:
        run_dir = Path(args.run_dir)
    else:
        h_dir = HYPOTHESIS_DIRS[args.eval_mode]
        run_dir = h_dir / "results" / "gepa_native"
    run_dir.mkdir(parents=True, exist_ok=True)

    # Load seed candidate
    seed_candidate = load_seed_candidate(args.eval_mode)
    print(f"Seed candidate files: {list(seed_candidate.keys())}")
    print(f"Eval mode: {args.eval_mode}")
    print(f"Run dir: {run_dir}")
    print(f"Max evaluations: {args.iterations}")

    # Setup LLM
    api_base, api_key = get_api_credentials()
    model = os.environ.get("GEPA_MODEL", "openai/aws/claude-opus-4-6")
    lm = make_litellm_lm(model, api_base=api_base, api_key=api_key, max_tokens=16384)

    # Build background context
    background = BACKGROUND_COMMON
    if args.eval_mode == "concurrent":
        background += "\n" + BACKGROUND_CONCURRENT

    # Module selector: "all" for multi-file (evolve together), "round_robin" for single
    module_selector = "all" if len(seed_candidate) > 1 else "round_robin"

    # Scores callback for orchestrator integration
    scores_callback = ScoresCallback(run_dir / "scores.jsonl")

    result = optimize_anything(
        seed_candidate=seed_candidate,
        evaluator=evaluate,
        objective=OBJECTIVES[args.eval_mode],
        background=background,
        config=GEPAConfig(
            engine=EngineConfig(
                run_dir=str(run_dir),
                max_metric_calls=args.iterations,
                capture_stdio=True,
                cache_evaluation=True,
                parallel=False,  # evaluator runs server — must be sequential
                display_progress_bar=True,
            ),
            reflection=ReflectionConfig(
                reflection_lm=lm,
                module_selector=module_selector,
            ),
            callbacks=[scores_callback],
        ),
    )

    print(f"\n{'='*70}")
    print("GEPA Native Optimization Complete")
    print(f"{'='*70}")
    best_score = max(result.val_aggregate_scores) if result.val_aggregate_scores else 0.0
    print(f"Best score: {best_score:.3f} GB/s")
    print(f"Candidates explored: {len(result.candidates)}")
    print(f"Results saved to: {run_dir}")

    # Write best candidate files
    best_dir = run_dir / "best_candidate"
    best_dir.mkdir(exist_ok=True)
    if isinstance(result.best_candidate, dict):
        for filename, content in result.best_candidate.items():
            (best_dir / filename).write_text(content)
    else:
        (best_dir / "pipeline.rs").write_text(result.best_candidate)
    print(f"Best candidate written to: {best_dir}")


if __name__ == "__main__":
    main()

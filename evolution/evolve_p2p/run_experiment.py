#!/usr/bin/env python3
"""P2P Evolution Experiment Orchestrator.

Runs all frameworks sequentially through the same evaluator with identical conditions.
Each framework gets: same seed, same scoring, same budget, same hardware config.

Usage:
    # All frameworks, 30 iterations each
    python run_experiment.py

    # Specific frameworks
    python run_experiment.py --frameworks gepa_native,adaevolve,random

    # Custom budget
    python run_experiment.py --iterations 50

    # Just one framework for testing
    python run_experiment.py --frameworks gepa_native --iterations 5

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
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

EXPERIMENT_DIR = Path(__file__).resolve().parent
REPO_ROOT = EXPERIMENT_DIR.parents[1]
EVO_FRAMEWORKS_ROOT = Path("/home/nara/certus/evo_frameworks")
GEPA_PROJECT = EVO_FRAMEWORKS_ROOT / "gepa"
SKYDISCOVER_PROJECT = EVO_FRAMEWORKS_ROOT / "skydiscover"
NOUS_ROOT = EVO_FRAMEWORKS_ROOT / "agentic-strategy-evolution"
AUTOSCIENTISTS_ROOT = EVO_FRAMEWORKS_ROOT / "AutoScientists"

EVALUATOR_PY = EXPERIMENT_DIR / "evaluator" / "evaluate_p2p.py"
INITIAL_PROGRAMS = EXPERIMENT_DIR / "initial_programs"
RESULTS_DIR = EXPERIMENT_DIR / "results"

# Frameworks grouped by interface type
GEPA_NATIVE_FRAMEWORKS = ["gepa_native"]
SKYDISCOVER_FRAMEWORKS = ["adaevolve", "evox", "openevolve"]
AGENTIC_FRAMEWORKS = ["nous", "autoscientists"]
ALL_FRAMEWORKS = GEPA_NATIVE_FRAMEWORKS + SKYDISCOVER_FRAMEWORKS + AGENTIC_FRAMEWORKS + ["random"]


def timestamp():
    return datetime.now().strftime("%Y-%m-%d_%H-%M-%S")


def run_framework(name: str, iterations: int) -> dict:
    """Run a single framework and return results summary."""
    run_dir = RESULTS_DIR / name
    run_dir.mkdir(parents=True, exist_ok=True)
    log_file = run_dir / f"run-{timestamp()}.log"

    print(f"\n{'='*70}")
    print(f"  Starting: {name} | iterations={iterations}")
    print(f"  Output: {run_dir}")
    print(f"{'='*70}\n")

    t_start = time.time()

    if name in GEPA_NATIVE_FRAMEWORKS:
        cmd = _build_gepa_native_cmd(iterations, run_dir)
    elif name in SKYDISCOVER_FRAMEWORKS:
        cmd = _build_skydiscover_cmd(name, iterations, run_dir)
    elif name == "nous":
        cmd = _build_nous_cmd(iterations, run_dir)
    elif name == "autoscientists":
        cmd = _build_autoscientists_cmd(iterations, run_dir)
    elif name == "random":
        cmd = _build_random_cmd(iterations, run_dir)
    else:
        return {"framework": name, "error": f"Unknown framework: {name}"}

    print(f"  CMD: {' '.join(str(c) for c in cmd)}\n")

    # Run framework
    timeout_s = 7200 if name in AGENTIC_FRAMEWORKS else iterations * 120
    env = os.environ.copy()

    try:
        with open(log_file, "w") as log:
            result = subprocess.run(
                cmd, stdout=log, stderr=subprocess.STDOUT,
                timeout=timeout_s, env=env,
            )
        returncode = result.returncode
    except subprocess.TimeoutExpired:
        returncode = -1
        with open(log_file, "a") as log:
            log.write(f"\nTIMEOUT after {timeout_s}s\n")
    except Exception as e:
        returncode = -2
        with open(log_file, "a") as log:
            log.write(f"\nERROR: {e}\n")

    t_end = time.time()
    wall_time = t_end - t_start

    # Parse scores
    scores = _parse_scores(run_dir)

    summary = {
        "framework": name,
        "iterations_completed": len(scores),
        "iterations_requested": iterations,
        "wall_time_seconds": round(wall_time, 1),
        "best_score": round(max(scores), 4) if scores else 0.0,
        "mean_score": round(sum(scores) / len(scores), 4) if scores else 0.0,
        "scores": scores,
        "returncode": returncode,
        "timestamp": timestamp(),
        "log_file": str(log_file),
    }

    summary_file = run_dir / "summary.json"
    summary_file.write_text(json.dumps(summary, indent=2))

    print(f"\n  {name} complete: {len(scores)} iterations, "
          f"best={summary['best_score']:.4f}, wall={wall_time:.0f}s\n")
    return summary


def _build_gepa_native_cmd(iterations: int, run_dir: Path) -> list[str]:
    """GEPA native: multi-file dict evolution."""
    runner = EXPERIMENT_DIR / "run_gepa_p2p.py"
    return [
        "uv", "run", "--project", str(GEPA_PROJECT),
        "python", str(runner),
        "--iterations", str(iterations),
        "--run-dir", str(run_dir),
    ]


def _build_skydiscover_cmd(algo: str, iterations: int, run_dir: Path) -> list[str]:
    """SkyDiscover frameworks: slim concatenated file, full-rewrite mode."""
    # Build concatenated seed if not exists
    concat_seed = _ensure_concatenated_seed()

    # Map framework name to SkyDiscover search algo
    search_map = {
        "adaevolve": "adaevolve",
        "evox": "evox",
        "openevolve": "openevolve_native",
    }
    search_algo = search_map.get(algo, algo)

    config_file = EXPERIMENT_DIR / "configs" / "skydiscover" / "config.yaml"
    _ensure_skydiscover_config(config_file, iterations)

    return [
        "uv", "run", "--project", str(SKYDISCOVER_PROJECT),
        "skydiscover-run",
        str(concat_seed),
        str(EVALUATOR_PY),
        "--search", search_algo,
        "--config", str(config_file),
        "-i", str(iterations),
        "-o", str(run_dir / "output"),
    ]


def _build_nous_cmd(iterations: int, run_dir: Path) -> list[str]:
    """Nous: agentic campaign runner."""
    config_file = EXPERIMENT_DIR / "configs" / "nous" / "campaign.yaml"
    _ensure_nous_config(config_file)

    return [
        "/usr/bin/python3.12",
        str(NOUS_ROOT / "run_campaign.py"),
        str(config_file),
        "--max-iterations", str(min(iterations, 3)),
        "--auto-approve",
        "--run-id", f"p2p-evolve-{timestamp()}",
        "--timeout", "7200",
    ]


def _build_autoscientists_cmd(iterations: int, run_dir: Path) -> list[str]:
    """AutoScientists: multi-agent collaborative."""
    task_dir = EXPERIMENT_DIR / "configs" / "autoscientists"
    _ensure_autoscientists_config(task_dir)

    return [
        "claude", "-p",
        f"Read runbook.md and execute. Task: {task_dir}. Run name: p2p_evolve_{timestamp()}.",
    ]


def _build_random_cmd(iterations: int, run_dir: Path) -> list[str]:
    """Random search: random constant perturbations as control."""
    random_script = EXPERIMENT_DIR / "random_search.py"
    _ensure_random_script(random_script)

    return [
        "python3", str(random_script),
        "--iterations", str(iterations),
        "--run-dir", str(run_dir),
    ]


def _parse_scores(run_dir: Path) -> list[float]:
    """Parse scores from framework output."""
    scores = []

    # Check scores.jsonl (our standard format)
    scores_file = run_dir / "scores.jsonl"
    if scores_file.exists():
        for line in scores_file.read_text().splitlines():
            try:
                entry = json.loads(line)
                score = entry.get("combined_score", 0.0)
                if score > 0:
                    scores.append(float(score))
            except (json.JSONDecodeError, TypeError, ValueError):
                continue

    # Check SkyDiscover output
    output_dir = run_dir / "output"
    if output_dir.exists():
        for jsonl_file in output_dir.rglob("*.jsonl"):
            for line in jsonl_file.read_text().splitlines():
                try:
                    entry = json.loads(line)
                    score = entry.get("combined_score") or entry.get("score", 0.0)
                    if score > 0:
                        scores.append(float(score))
                except (json.JSONDecodeError, TypeError, ValueError):
                    continue

    return scores


# --- Config/seed generation helpers ---

def _ensure_concatenated_seed() -> Path:
    """Build slim concatenated seed for SkyDiscover frameworks."""
    concat_path = EXPERIMENT_DIR / "initial_programs" / "concatenated_seed.rs"
    if concat_path.exists():
        return concat_path

    pipeline = (INITIAL_PROGRAMS / "pipeline.rs").read_text()
    dma = (INITIAL_PROGRAMS / "dma.rs").read_text()

    # Extract PipelineRing::new from lib.rs (first ~60 lines are the relevant part)
    lib_content = (INITIAL_PROGRAMS / "lib.rs").read_text()
    # Find the PipelineRing section
    lines = lib_content.split("\n")
    ring_lines = []
    in_pipeline_mod = False
    for line in lines:
        if "pub mod pipeline;" in line or "pub use pipeline" in line:
            continue
        if "use crate::pipeline" in line or "pipeline::" in line:
            ring_lines.append(line)

    # For SkyDiscover: include full pipeline.rs + key dma.rs functions
    content = f"""// --- FILE: pipeline.rs ---
{pipeline}

// --- FILE: dma.rs (buffer creation functions) ---
{dma}
"""
    concat_path.write_text(content)
    return concat_path


def _ensure_skydiscover_config(config_file: Path, iterations: int):
    """Create SkyDiscover config if not exists."""
    config_file.parent.mkdir(parents=True, exist_ok=True)
    if config_file.exists():
        return

    config_file.write_text(f"""language: rust
max_iterations: {iterations}
diff_based_generation: false

llm:
  api_base: "https://ete-litellm.ai-models.vpc-int.res.ibm.com"
  models:
    - name: "aws/claude-opus-4-6"
      weight: 1.0
  max_tokens: 16384
  timeout: 300

optimization:
  objective: "Maximize cold-lookup throughput and minimize tail latency for SSD-to-GPU transfer"

evaluator:
  timeout: 90

prompt:
  system_message: |
    You are optimizing a storage server's NVMe-SSD-to-GPU data transfer path.
    The score rewards higher throughput (GB/s) and lower p99 latency.
    Hardware: NVMe Gen4 SSDs via SPDK, NVIDIA A30 GPU PCIe Gen4 x16.
    Kernel modules loaded: nvidia-peermem, gdrdrv.
    The gpu-services crate has DMA buffer creation functions for various memory types.
    Data integrity is mandatory — corruption scores -1.
""")


def _ensure_nous_config(config_file: Path):
    """Create Nous campaign config if not exists."""
    config_file.parent.mkdir(parents=True, exist_ok=True)
    if config_file.exists():
        return

    config_file.write_text("""name: p2p-evolution
hypothesis: "The storage server's SSD-to-GPU cold lookup throughput can be significantly improved by optimizing the data transfer pipeline."
target_binary: certus-server
build_command: "cargo build -p certus-server --release"
benchmark_command: "python3 apps/python/certus-api-bench.py --server localhost:50051 --clients 1 --num-objects 16 --iterations 10 --block-size 4194304"
metric: "cold lookup aggregate GB/s"
direction: maximize
files_in_scope:
  - components/dispatcher/src/pipeline.rs
  - components/dispatcher/src/lib.rs
  - components/gpu-services/src/dma.rs
constraints:
  - "Must compile: cargo build -p certus-server --release"
  - "Data integrity must pass (no ERRORS in benchmark output)"
  - "Must not modify interfaces or gRPC service signatures"
""")


def _ensure_autoscientists_config(task_dir: Path):
    """Create AutoScientists TASK.md if not exists."""
    task_dir.mkdir(parents=True, exist_ok=True)
    task_md = task_dir / "TASK.md"
    if task_md.exists():
        return

    task_md.write_text("""---
task_type: optimization
name: p2p-storage-gpu-transfer
---

# Optimize SSD-to-GPU Data Transfer

## Goal
Maximize cold-lookup throughput (GB/s) and minimize p99 latency (ms) for the
certus-server storage system's NVMe-SSD-to-GPU data path.

## Evaluation
- Build: `cargo build -p certus-server --release`
- Run server: `./target/release/certus-server --device-pci 0000:62:00.0 --format`
- Benchmark: `python3 apps/python/certus-api-bench.py --server localhost:50051 --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- Score: 0.60 * (throughput / 12.0) + 0.40 * (0.4 / p99_ms)
- Hard constraint: no ERRORS in benchmark output

## Files in scope
- components/dispatcher/src/pipeline.rs
- components/dispatcher/src/lib.rs
- components/gpu-services/src/dma.rs

## Hardware
- NVMe Gen4 SSDs via SPDK userspace driver
- NVIDIA A30 GPU, PCIe Gen4 x16
- Kernel modules: nvidia-peermem, gdrdrv
- Current baseline: ~2.4 GB/s cold lookup, score ~0.20

## Constraints
- Must compile
- Data integrity must pass
- Do not modify interfaces, gRPC service, or benchmark client
""")


def _ensure_random_script(script_path: Path):
    """Create random search script if not exists."""
    if script_path.exists():
        return

    script_path.write_text('''#!/usr/bin/env python3
"""Random search baseline: random constant perturbations in pipeline.rs."""
import argparse
import json
import os
import random
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "evaluator"))
from evaluate_p2p import evaluate

INITIAL = Path(__file__).resolve().parent / "initial_programs" / "pipeline.rs"


def random_mutate(code: str) -> str:
    """Randomly perturb numeric constants in the code."""
    def replace_const(match):
        val = int(match.group(0))
        if random.random() < 0.3:  # 30% chance to mutate each constant
            factor = random.choice([0.5, 0.75, 1.5, 2.0, 4.0])
            new_val = max(1, int(val * factor))
            return str(new_val)
        return match.group(0)

    return re.sub(r"\\b([1-9]\\d{0,5})\\b", replace_const, code)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--run-dir", type=str, required=True)
    args = parser.parse_args()

    run_dir = Path(args.run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    scores_file = run_dir / "scores.jsonl"

    seed = INITIAL.read_text()
    best_score = 0.0

    for i in range(args.iterations):
        candidate = random_mutate(seed)
        score, metrics = evaluate(candidate)
        with open(scores_file, "a") as f:
            f.write(json.dumps({"combined_score": round(score, 4), "iteration": i}) + "\\n")
        if score > best_score:
            best_score = score
            (run_dir / "best_pipeline.rs").write_text(candidate)
        print(f"  [{i+1}/{args.iterations}] score={score:.4f} best={best_score:.4f}")

    print(f"\\nRandom search complete. Best: {best_score:.4f}")


if __name__ == "__main__":
    main()
''')


def main():
    parser = argparse.ArgumentParser(description="P2P Evolution Experiment Orchestrator")
    parser.add_argument(
        "--frameworks", type=str, default=None,
        help=f"Comma-separated frameworks to run (default: all). Available: {','.join(ALL_FRAMEWORKS)}",
    )
    parser.add_argument(
        "--iterations", type=int, default=30,
        help="Iterations per framework (default: 30). Nous/AutoScientists use min(iterations, 3) deep iterations.",
    )
    args = parser.parse_args()

    frameworks = args.frameworks.split(",") if args.frameworks else ALL_FRAMEWORKS

    print(f"{'='*70}")
    print(f"  P2P Evolution Experiment")
    print(f"{'='*70}")
    print(f"  Frameworks: {frameworks}")
    print(f"  Iterations: {args.iterations}")
    print(f"  Results: {RESULTS_DIR}")
    print(f"  Evaluator: {EVALUATOR_PY}")
    print(f"  Scoring: 60% throughput + 40% latency")
    print(f"  Data PCI: {os.environ.get('CERTUS_DATA_PCI', '0000:62:00.0')}")
    print()

    summaries = []
    for name in frameworks:
        if name not in ALL_FRAMEWORKS:
            print(f"  WARNING: Unknown framework '{name}', skipping")
            continue
        summary = run_framework(name, args.iterations)
        summaries.append(summary)

    # Final summary
    print(f"\n{'='*70}")
    print(f"  EXPERIMENT COMPLETE")
    print(f"{'='*70}\n")
    print(f"  {'Framework':<16} {'Iters':>6} {'Best':>8} {'Mean':>8} {'Wall(s)':>8}")
    print(f"  {'-'*50}")
    for s in summaries:
        print(f"  {s['framework']:<16} {s['iterations_completed']:>6} "
              f"{s['best_score']:>8.4f} {s['mean_score']:>8.4f} {s['wall_time_seconds']:>8.0f}")

    # Save overall summary
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    (RESULTS_DIR / "experiment_summary.json").write_text(
        json.dumps(summaries, indent=2)
    )


if __name__ == "__main__":
    main()

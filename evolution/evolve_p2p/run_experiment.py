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
KSEARCH_ROOT = EVO_FRAMEWORKS_ROOT / "K-Search"

EVALUATOR_PY = EXPERIMENT_DIR / "evaluator" / "evaluate_p2p.py"
INITIAL_PROGRAMS = EXPERIMENT_DIR / "initial_programs"
RESULTS_DIR = EXPERIMENT_DIR / "results"
CONFIGS_DIR = EXPERIMENT_DIR / "configs"
CONFIGS_HINT_DIR = EXPERIMENT_DIR / "configs_hint"
RESULTS_HINT_DIR = EXPERIMENT_DIR / "results_hint"

# Frameworks grouped by interface type
GEPA_NATIVE_FRAMEWORKS = ["gepa_native"]
SKYDISCOVER_FRAMEWORKS = ["adaevolve", "evox", "openevolve", "shinkaevolve"]
AGENTIC_FRAMEWORKS = ["nous", "autoscientists", "coding_agent"]
WORLD_MODEL_FRAMEWORKS = ["ksearch"]
ALL_FRAMEWORKS = (GEPA_NATIVE_FRAMEWORKS + SKYDISCOVER_FRAMEWORKS +
                  WORLD_MODEL_FRAMEWORKS + AGENTIC_FRAMEWORKS)


def timestamp():
    return datetime.now().strftime("%Y-%m-%d_%H-%M-%S")


def run_framework(name: str, iterations: int, hint_configs: Path | None = None) -> dict:
    """Run a single framework and return results summary."""
    run_dir = RESULTS_DIR / name
    run_dir.mkdir(parents=True, exist_ok=True)
    log_file = run_dir / f"run-{timestamp()}.log"

    print(f"\n{'='*70}")
    print(f"  Starting: {name} | iterations={iterations}")
    print(f"  Output: {run_dir}")
    print(f"{'='*70}\n")

    t_start = time.time()

    # Use main repo directly (no worktree — SPDK deps are untracked and can't be symlinked reliably)
    # Restore source files to wild-type before each framework run
    for init_file in INITIAL_PROGRAMS.glob("*.rs"):
        target_map = {
            "pipeline.rs": REPO_ROOT / "components" / "dispatcher" / "src" / "pipeline.rs",
            "lib.rs": REPO_ROOT / "components" / "dispatcher" / "src" / "lib.rs",
            "dma.rs": REPO_ROOT / "components" / "gpu-services" / "src" / "dma.rs",
        }
        target = target_map.get(init_file.name)
        if target and target.exists():
            import shutil
            shutil.copy2(init_file, target)

    run_repo_root = REPO_ROOT
    run_evaluator = EVALUATOR_PY

    if name in GEPA_NATIVE_FRAMEWORKS:
        cmd = _build_gepa_native_cmd(iterations, run_dir, run_repo_root, hint_configs=hint_configs)
    elif name in SKYDISCOVER_FRAMEWORKS:
        cmd = _build_skydiscover_cmd(name, iterations, run_dir, run_repo_root, hint_configs=hint_configs)
    elif name == "ksearch":
        cmd = _build_ksearch_cmd(iterations, run_dir, run_repo_root, hint_configs=hint_configs)
    elif name == "nous":
        cmd = _build_nous_cmd(iterations, run_dir, run_repo_root, hint_configs=hint_configs)
    elif name == "autoscientists":
        cmd = _build_autoscientists_cmd(iterations, run_dir, run_repo_root, hint_configs=hint_configs)
    elif name == "coding_agent":
        cmd = _build_coding_agent_cmd(iterations, run_dir, run_repo_root, hint_configs=hint_configs)
    else:
        return {"framework": name, "error": f"Unknown framework: {name}"}

    print(f"  CMD: {' '.join(str(c) for c in cmd)}\n")

    # Run framework
    timeout_s = 7200 if name in AGENTIC_FRAMEWORKS else iterations * 300
    env = os.environ.copy()
    env["CERTUS_REPO_ROOT"] = str(run_repo_root)
    # Pass hint background to GEPA via env var
    if hint_configs:
        bg_file = hint_configs / "gepa_background.txt"
        if bg_file.exists():
            env["GEPA_BACKGROUND"] = bg_file.read_text()
        def_file = hint_configs / "ksearch_definition.txt"
        if def_file.exists():
            env["KSEARCH_DEFINITION_FILE"] = str(def_file)
    # Ensure API key is available from bakeoff key file if not in env
    if "LITELLM_API_KEY" not in env and "OPENAI_API_KEY" not in env:
        key_path = "/tmp/.bakeoff_key"
        if os.path.exists(key_path):
            with open(key_path) as f:
                env["LITELLM_API_KEY"] = f.read().strip()
    # SkyDiscover/K-Search use OpenAI client; map our LiteLLM key
    if "LITELLM_API_KEY" in env and "OPENAI_API_KEY" not in env:
        env["OPENAI_API_KEY"] = env["LITELLM_API_KEY"]
    if "LITELLM_API_BASE" in env and "OPENAI_BASE_URL" not in env:
        env["OPENAI_BASE_URL"] = env.get(
            "LITELLM_API_BASE", "https://ete-litellm.ai-models.vpc-int.res.ibm.com"
        )

    cwd = str(run_repo_root)

    try:
        with open(log_file, "w") as log:
            result = subprocess.run(
                cmd, stdout=log, stderr=subprocess.STDOUT,
                timeout=timeout_s, env=env, cwd=cwd,
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
    finally:
        # Restore source files to wild-type (even on crash/kill)
        import shutil as _shutil
        target_map = {
            "pipeline.rs": REPO_ROOT / "components" / "dispatcher" / "src" / "pipeline.rs",
            "lib.rs": REPO_ROOT / "components" / "dispatcher" / "src" / "lib.rs",
            "dma.rs": REPO_ROOT / "components" / "gpu-services" / "src" / "dma.rs",
        }
        for init_file in INITIAL_PROGRAMS.glob("*.rs"):
            target = target_map.get(init_file.name)
            if target:
                _shutil.copy2(init_file, target)

    t_end = time.time()
    wall_time = t_end - t_start

    # For SkyDiscover: extract best program from checkpoint to best/ dir
    if name in SKYDISCOVER_FRAMEWORKS:
        _extract_skydiscover_best(run_dir)

    # For ShinkaEvolve: extract scores from SQLite DB
    if name == "shinkaevolve":
        _extract_shinkaevolve_scores(run_dir)

    # For Nous: extract scores from findings.json files
    if name == "nous":
        _extract_nous_scores(run_dir)

    # For AutoScientists/coding_agent: extract scores from log (claude -p output)
    if name in ("autoscientists", "coding_agent"):
        _extract_autoscientists_scores(run_dir)

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

    # Post-run analysis
    _analyze_run(name, run_dir, scores, log_file)

    print(f"\n  {name} complete: {len(scores)} iterations, "
          f"best={summary['best_score']:.4f}, wall={wall_time:.0f}s\n")
    return summary


def _build_gepa_native_cmd(iterations: int, run_dir: Path, repo_root: Path, hint_configs: Path | None = None) -> list[str]:
    """GEPA native: multi-file dict evolution."""
    runner = EXPERIMENT_DIR / "run_gepa_p2p.py"
    cmd = [
        "uv", "run", "--project", str(GEPA_PROJECT),
        "python", str(runner),
        "--iterations", str(iterations),
        "--run-dir", str(run_dir),
    ]
    # GEPA uses CERTUS_REPO_ROOT env var (set in run_framework) for repo path
    # Background context for hints: GEPA reads GEPA_BACKGROUND env var
    return cmd


def _build_skydiscover_cmd(algo: str, iterations: int, run_dir: Path, repo_root: Path, hint_configs: Path | None = None) -> list[str]:
    """SkyDiscover frameworks: slim concatenated file, full-rewrite mode."""
    # Build concatenated seed if not exists
    concat_seed = _ensure_concatenated_seed()

    # Map framework name to SkyDiscover search algo
    search_map = {
        "adaevolve": "adaevolve",
        "evox": "evox",
        "openevolve": "openevolve_native",
        "shinkaevolve": "shinkaevolve",
    }
    search_algo = search_map.get(algo, algo)

    if hint_configs:
        config_file = hint_configs / "skydiscover" / "config.yaml"
    else:
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


def _build_nous_cmd(iterations: int, run_dir: Path, repo_root: Path, hint_configs: Path | None = None) -> list[str]:
    """Nous: agentic campaign runner."""
    if hint_configs:
        # Use pre-built hint campaign config
        config_file = hint_configs / "nous" / "campaign.yaml"
    else:
        config_file = run_dir / "campaign.yaml"
        _ensure_nous_config(config_file, repo_path=str(repo_root))

    return [
        "/usr/bin/python3.12",
        str(NOUS_ROOT / "run_campaign.py"),
        str(config_file),
        "--max-iterations", str(min(iterations, 3)),
        "--auto-approve",
        "--run-id", f"p2p-evolve-{timestamp()}",
        "--timeout", "7200",
    ]


def _build_autoscientists_cmd(iterations: int, run_dir: Path, repo_root: Path, hint_configs: Path | None = None) -> list[str]:
    """AutoScientists: direct optimization via Claude with full permissions."""
    if hint_configs:
        task_dir = hint_configs / "autoscientists"
    else:
        task_dir = EXPERIMENT_DIR / "configs" / "autoscientists"
        _ensure_autoscientists_config(task_dir)

    task_md = task_dir / "TASK.md"
    evaluator_path = str(repo_root / "evolution" / "evolve_p2p" / "evaluator" / "evaluate_p2p.py")
    repo_root_str = str(repo_root)

    prompt = (
        f"You are an optimization agent. Your goal is to maximize the P2P SSD-to-GPU "
        f"data transfer score for certus-server.\n\n"
        f"## Task\n{task_md.read_text()}\n\n"
        f"## Iteration Loop\n"
        f"Repeat up to {min(iterations, 10)} times:\n"
        f"1. Read the target files (components/dispatcher/src/pipeline.rs, "
        f"components/dispatcher/src/lib.rs, components/gpu-services/src/dma.rs)\n"
        f"2. Design an optimization (one change per iteration)\n"
        f"3. Edit the file(s)\n"
        f"4. Evaluate: python3 {evaluator_path} --test\n"
        f"5. Record the score. If it regressed, revert and try a different approach.\n"
        f"6. After all iterations, print a JSON summary to stdout:\n"
        f'   {{"iterations": N, "best_score": X, "scores": [...]}}\n\n'
        f"## Rules\n"
        f"- The code MUST compile (cargo build -p certus-server --release)\n"
        f"- Data integrity must pass\n"
        f"- Do not modify interfaces, gRPC service, or the benchmark client\n"
        f"- Current baseline score: ~0.20 (2.4 GB/s cold lookup)\n"
        f"- Scoring: 0.60 * (throughput_gbps / 12.0) + 0.40 * (0.4 / p99_ms)\n"
        f"- Work in {repo_root_str}\n"
    )

    return [
        "claude", "-p", prompt,
        "--dangerously-skip-permissions",
        "--add-dir", repo_root_str,
        "--add-dir", str(run_dir),
    ]


def _build_coding_agent_cmd(iterations: int, run_dir: Path, repo_root: Path, hint_configs: Path | None = None) -> list[str]:
    """Coding agent: sequential iterate-with-feedback optimization."""
    evaluator_path = str(repo_root / "evolution" / "evolve_p2p" / "evaluator" / "evaluate_p2p.py")
    repo_root_str = str(repo_root)

    # Load P2P hint block if in hint mode
    hint_block = ""
    if hint_configs:
        hints_md = hint_configs / "HINTS.md"
        if hints_md.exists():
            # Extract the Implementation Guide section
            import re as _re
            content = hints_md.read_text()
            guide_match = _re.search(
                r"## Direction\n(.*?)## Compile Barriers",
                content, _re.DOTALL,
            )
            if guide_match:
                hint_block = (
                    "\n## Target Optimization\n"
                    + guide_match.group(1).strip() + "\n\n"
                    "## Implementation Notes\n"
                    "- Build uses --features p2p, so all #[cfg(feature = \"p2p\")] code is compiled.\n"
                    "- The cuda_ffi module uses std::os::raw::c_int for all flags and error return types.\n"
                    "- The function create_spdk_dma_buffer_from_gpu_bar(gpu_ptr, size, container_fd) in dma.rs creates an SPDK-registered DMA buffer backed by GPU BAR1 memory.\n"
                    "- PipelineRing.streams is [GpuStream; 2] — callers pass &ring.streams as a fixed-size array.\n\n"
                )

    prompt = (
        f"You are a performance optimization agent. Maximize the cold-lookup "
        f"SSD-to-GPU data transfer score for certus-server.\n\n"
        f"## Scoring\n"
        f"score = 0.60 * (throughput_gbps / 12.0) + 0.40 * (0.4 / p99_ms)\n"
        f"Baseline: ~0.20 (2.4 GB/s, 1.9ms p99).\n\n"
        f"## Hardware\n"
        f"- NVMe Gen4 SSD via SPDK userspace driver\n"
        f"- NVIDIA A30 GPU, PCIe Gen4 x16\n"
        f"- Kernel modules: nvidia-peermem, gdrdrv\n"
        f"- 2048 hugepages, memlock unlimited, VFIO-bound NVMe\n\n"
        + hint_block +
        f"## Evaluation\n"
        f"- Build: cargo build -p certus-server --release --features p2p\n"
        f"- Evaluate: python3 {evaluator_path} --test\n"
        f"  (this builds, starts server, benchmarks, checks integrity, prints score)\n"
        f"- The evaluator scores whatever is currently in the source files\n\n"
        f"## Files in scope\n"
        f"- components/dispatcher/src/pipeline.rs\n"
        f"- components/dispatcher/src/lib.rs\n"
        f"- components/gpu-services/src/dma.rs\n\n"
        f"## Iteration Loop\n"
        f"Repeat up to {min(iterations, 10)} times:\n"
        f"1. Read the source files and understand the current implementation\n"
        f"2. Identify a bottleneck or optimization opportunity\n"
        f"3. Make ONE focused change\n"
        f"4. Save your candidate: copy the modified files to "
        f"{run_dir}/candidates/gen_N/ (where N is iteration number starting at 0). "
        f"Save as pipeline.rs, lib.rs, dma.rs (only the files you changed).\n"
        f"5. Run the evaluator and observe the score\n"
        f"6. If it regressed, revert and try a different approach\n"
        f"7. Reason about what you learned before the next iteration\n\n"
        f"## Constraints\n"
        f"- Code must compile with --features p2p\n"
        f"- Data integrity must pass (no ERRORS in benchmark output)\n"
        f"- Do not modify gRPC service, benchmark client, or interfaces\n\n"
        f"## Output\n"
        f"After all iterations, print a JSON summary:\n"
        f'{{"iterations": N, "best_score": X, "scores": [s1, s2, ...]}}\n\n'
        f"Then a markdown table: #, Change, Score, Result (GB/s and ms p99).\n"
    )

    return [
        "claude", "-p", prompt,
        "--output-format", "json",
        "--dangerously-skip-permissions",
        "--add-dir", repo_root_str,
        "--add-dir", str(run_dir),
    ]


def _build_ksearch_cmd(iterations: int, run_dir: Path, repo_root: Path, hint_configs: Path | None = None) -> list[str]:
    """K-Search: world-model-guided optimization."""
    api_base = os.environ.get(
        "LITELLM_API_BASE", "https://ete-litellm.ai-models.vpc-int.res.ibm.com"
    )
    api_key = os.environ.get("LITELLM_API_KEY", "")
    if not api_key:
        key_path = "/tmp/.bakeoff_key"
        if os.path.exists(key_path):
            with open(key_path) as f:
                api_key = f.read().strip()

    model = os.environ.get("GEPA_MODEL", "aws/claude-opus-4-6")

    runner_script = EXPERIMENT_DIR / "run_ksearch_p2p.py"

    cmd = [
        "/usr/bin/python3.12", str(runner_script),
        "--iterations", str(iterations),
        "--run-dir", str(run_dir),
        "--model", model,
        "--api-base", api_base,
        "--api-key", api_key,
    ]
    # K-Search uses KSEARCH_TASK_DEFINITION env var for hints (set in run_framework env)
    return cmd




def _extract_skydiscover_best(run_dir: Path):
    """After SkyDiscover finishes, find and save the best program source."""
    import re as _re
    best_dir = run_dir / "best"

    # Try checkpoint dir first
    output_dir = run_dir / "output"
    for best_file in sorted(output_dir.rglob("best_program*")):
        if best_file.suffix in (".rs", ""):
            content = best_file.read_text()
            if "// --- FILE:" in content:
                best_dir.mkdir(parents=True, exist_ok=True)
                # Split concatenated file into individual files
                marker_re = _re.compile(r"^//\s*---\s*FILE:\s*(\S+?)(?:\s*\(.*?\))?\s*---\s*$", _re.MULTILINE)
                markers = list(marker_re.finditer(content))
                if markers:
                    for i, m in enumerate(markers):
                        filename = m.group(1)
                        start = m.end()
                        end = markers[i + 1].start() if i + 1 < len(markers) else len(content)
                        (best_dir / filename).write_text(content[start:end].strip("\n"))
                else:
                    (best_dir / "pipeline.rs").write_text(content)
                return

    # Fallback: try to find best program in the log (SkyDiscover prints it)
    for log_file in sorted(run_dir.glob("run-*.log")):
        text = log_file.read_text()
        # SkyDiscover logs "Best program saved to: <path>" or similar
        match = _re.search(r"Best program.*?saved.*?:\s*(.+)", text)
        if match:
            saved_path = Path(match.group(1).strip())
            if saved_path.exists():
                content = saved_path.read_text()
                best_dir.mkdir(parents=True, exist_ok=True)
                if "// --- FILE:" in content:
                    marker_re = _re.compile(r"^//\s*---\s*FILE:\s*(\S+?)(?:\s*\(.*?\))?\s*---\s*$", _re.MULTILINE)
                    markers = list(marker_re.finditer(content))
                    for i, m in enumerate(markers):
                        filename = m.group(1)
                        start = m.end()
                        end = markers[i + 1].start() if i + 1 < len(markers) else len(content)
                        (best_dir / filename).write_text(content[start:end].strip("\n"))
                else:
                    (best_dir / "pipeline.rs").write_text(content)
                return


def _extract_autoscientists_scores(run_dir: Path):
    """Extract scores from AutoScientists/coding_agent log (claude -p output)."""
    import re as _re

    scores_file = run_dir / "scores.jsonl"
    entries = []

    for log_file in sorted(run_dir.glob("run-*.log")):
        try:
            raw_text = log_file.read_text()
        except OSError:
            continue

        # Handle --output-format json: extract result text and cost
        text = raw_text
        try:
            json_resp = json.loads(raw_text)
            if isinstance(json_resp, dict) and "result" in json_resp:
                text = json_resp["result"]
                # Write cost.json from the JSON response
                cost_usd = json_resp.get("total_cost_usd")
                usage = json_resp.get("usage", {})
                if cost_usd is not None:
                    cost_file = run_dir / "cost.json"
                    cost_file.write_text(json.dumps({
                        "total_cost_usd": round(cost_usd, 4),
                        "total_input_tokens": usage.get("input_tokens", 0)
                            + usage.get("cache_read_input_tokens", 0)
                            + usage.get("cache_creation_input_tokens", 0),
                        "total_output_tokens": usage.get("output_tokens", 0),
                        "duration_ms": json_resp.get("duration_ms"),
                    }))
        except (json.JSONDecodeError, ValueError, TypeError):
            pass

        # Look for benchmark output patterns: "per-client=X.XX GB/s" or "Throughput: X.XX GB/s"
        # or our evaluator output "[eval] Score: X.XXXX | Throughput: X.XX GB/s | p99: XXXXus"
        eval_re = _re.compile(
            r"Score:\s*([\d.]+)\s*\|\s*Throughput:\s*([\d.]+)\s*GB/s\s*\|\s*p99:\s*(\d+)us"
        )
        bench_re = _re.compile(r"per-client=([\d.]+)\s*GB/s")
        p99_re = _re.compile(r"p99=\s*([\d.]+)\s*us")

        for i, m in enumerate(eval_re.finditer(text)):
            score = float(m.group(1))
            throughput = float(m.group(2))
            p99_ms = float(m.group(3)) / 1000.0
            entries.append({
                "combined_score": round(score, 4),
                "iteration": i,
                "throughput_gbps": throughput,
                "p99_latency_ms": p99_ms,
                "build_succeeded": True,
                "failure_type": "success",
                "error": "",
            })

        # Fallback: parse raw benchmark output
        if not entries:
            for i, m in enumerate(bench_re.finditer(text)):
                throughput = float(m.group(1))
                p99_match = p99_re.search(text[m.end():m.end()+200])
                p99_ms = float(p99_match.group(1)) / 1000.0 if p99_match else None
                score = 0.0
                if throughput > 0 and p99_ms and p99_ms > 0:
                    score = round(0.6 * min(1.0, throughput / 12.0) + 0.4 * min(1.0, 0.4 / p99_ms), 4)
                entries.append({
                    "combined_score": score,
                    "iteration": i,
                    "throughput_gbps": throughput,
                    "p99_latency_ms": p99_ms,
                    "build_succeeded": score > 0,
                    "failure_type": "success" if score > 0 else "other_failure",
                    "error": "",
                })

        # Fallback 2: parse JSON summary + markdown table for throughput/latency
        # JSON: {"iterations": N, "best_score": X, "scores": [...]}
        # Table: "| N | description | score | X.XX GB/s, Y.YYms p99 |"
        # Freeform: "Score 0.3374, 3.89 GB/s" or "Score 0.3374, 3.89 GB/s - description"
        # Final line: "X.XX → Y.YY GB/s throughput, A.AA → B.BBms p99"
        if not entries:
            json_re = _re.compile(r'\{"iterations":\s*\d+.*?"scores":\s*\[([^\]]+)\]\}')
            table_re = _re.compile(
                r"\|\s*\d+b?\s*\|[^|]+\|\s*\*{0,2}([\d.]+)\*{0,2}\s*\|.*?([\d.]+)\s*GB/s.*?([\d.]+)\s*ms"
            )
            # Parse final summary line for best metrics
            final_re = _re.compile(
                r"([\d.]+)\s*→\s*([\d.]+)\s*GB/s.*?([\d.]+)\s*→\s*([\d.]+)\s*ms\s*p99"
            )
            # Also match freeform "Score X.XXXX, Y.YY GB/s" lines
            freeform_re = _re.compile(
                r"Score\s+([\d.]+),?\s*([\d.]+)\s*GB/s(?:.*?([\d.]+)\s*ms)?"
            )
            table_data = {}
            for m in table_re.finditer(text):
                score = float(m.group(1))
                table_data[round(score, 4)] = {
                    "throughput_gbps": float(m.group(2)),
                    "p99_latency_ms": float(m.group(3)),
                }
            for m in freeform_re.finditer(text):
                score = float(m.group(1))
                table_data[round(score, 4)] = {
                    "throughput_gbps": float(m.group(2)),
                    "p99_latency_ms": float(m.group(3)) if m.group(3) else None,
                }
            # Extract best throughput/p99 from final summary
            final_match = final_re.search(text)
            best_throughput = float(final_match.group(2)) if final_match else None
            best_p99 = float(final_match.group(4)) if final_match else None

            m = json_re.search(text)
            if m:
                try:
                    summary = json.loads(m.group(0))
                    scores_list = summary.get("scores", [])
                    max_score = max(scores_list) if scores_list else 0
                    for i, score in enumerate(scores_list):
                        rounded = round(score, 4)
                        td = table_data.get(rounded, {})
                        throughput = td.get("throughput_gbps")
                        p99_ms = td.get("p99_latency_ms")
                        # Use final summary values for the best score
                        if not throughput and score == max_score:
                            throughput = best_throughput
                            p99_ms = best_p99
                        entries.append({
                            "combined_score": rounded,
                            "iteration": i,
                            "throughput_gbps": throughput,
                            "p99_latency_ms": p99_ms,
                            "build_succeeded": score > 0,
                            "failure_type": "success" if score > 0 else "build_failure",
                            "error": "",
                        })
                except (json.JSONDecodeError, ZeroDivisionError):
                    pass

    if entries:
        with open(scores_file, "w") as f:
            for e in entries:
                f.write(json.dumps(e) + "\n")

    # Extract cost from claude session stats if available
    cost_file = run_dir / "cost.json"
    if not cost_file.exists():
        for log_file in sorted(run_dir.glob("run-*.log")):
            try:
                text = log_file.read_text()
            except OSError:
                continue
            # Claude -p prints token usage at the end in some configs
            input_m = _re.search(r"input[_ ]tokens?[:\s]*([\d,]+)", text, _re.IGNORECASE)
            output_m = _re.search(r"output[_ ]tokens?[:\s]*([\d,]+)", text, _re.IGNORECASE)
            if input_m and output_m:
                inp = int(input_m.group(1).replace(",", ""))
                out = int(output_m.group(1).replace(",", ""))
                cost = inp * 15.0 / 1_000_000 + out * 75.0 / 1_000_000
                cost_file.write_text(json.dumps({
                    "total_cost_usd": round(cost, 4),
                    "total_input_tokens": inp,
                    "total_output_tokens": out,
                }))
                break
        # Estimate cost from wall time if no token info available
        if not cost_file.exists():
            summary_file = run_dir / "summary.json"
            if summary_file.exists():
                try:
                    summary = json.loads(summary_file.read_text())
                    wall_s = summary.get("wall_time_seconds", 0)
                    est_output_tokens = int(wall_s * 80)
                    est_input_tokens = est_output_tokens * 4
                    cost = est_input_tokens * 15.0 / 1_000_000 + est_output_tokens * 75.0 / 1_000_000
                    cost_file.write_text(json.dumps({
                        "total_cost_usd": round(cost, 4),
                        "total_input_tokens": est_input_tokens,
                        "total_output_tokens": est_output_tokens,
                        "estimated": True,
                    }))
                except (json.JSONDecodeError, OSError):
                    pass


def _extract_nous_scores(run_dir: Path):
    """Extract scores from Nous findings.json files into scores.jsonl."""
    import re as _re
    import glob as _glob

    # Nous stores results in .nous/<run-id>/runs/iter-N/findings.json
    repo_root = EXPERIMENT_DIR.parents[1]

    # Find the campaign dir matching this run's timestamp
    # run_dir name is like "nous/" and the log has the timestamp
    all_campaigns = sorted(repo_root.glob(".nous/p2p-evolve-*"))
    if not all_campaigns:
        return

    # Match campaign to this run by finding the latest campaign that started
    # at or before the run's log timestamp
    run_log = None
    for log_file in run_dir.glob("run-*.log"):
        run_log = log_file
        break

    if run_log:
        # Extract timestamp from log filename: run-2026-06-02_21-41-43.log
        log_ts = run_log.stem.replace("run-", "")  # "2026-06-02_21-41-43"
        # Find the campaign dir with matching timestamp
        matching_campaign = None
        for campaign_dir in all_campaigns:
            campaign_ts = campaign_dir.name.replace("p2p-evolve-", "")
            if campaign_ts == log_ts:
                matching_campaign = campaign_dir
                break
        if not matching_campaign:
            # Fallback: use the most recent campaign
            matching_campaign = all_campaigns[-1]
    else:
        matching_campaign = all_campaigns[-1]

    nous_dirs = sorted(matching_campaign.glob("runs/iter-*/findings.json"))

    # Also extract total cost from llm_metrics.jsonl (only from matching campaign)
    total_cost = 0.0
    total_input_tokens = 0
    total_output_tokens = 0
    metrics_file = matching_campaign / "llm_metrics.jsonl"
    if metrics_file.exists():
        try:
            for line in metrics_file.read_text().splitlines():
                entry = json.loads(line)
                total_cost += entry.get("cost_usd", 0) or 0
                total_input_tokens += entry.get("input_tokens", 0) or 0
                total_output_tokens += entry.get("output_tokens", 0) or 0
        except (json.JSONDecodeError, ValueError):
            pass
    if total_cost > 0:
        cost_file = run_dir / "cost.json"
        cost_file.write_text(json.dumps({
            "total_cost_usd": round(total_cost, 4),
            "total_input_tokens": total_input_tokens,
            "total_output_tokens": total_output_tokens,
        }, indent=2))

    if not nous_dirs:
        return

    scores_file = run_dir / "scores.jsonl"
    with open(scores_file, "w") as f:
        for i, findings_path in enumerate(nous_dirs):
            try:
                findings = json.loads(findings_path.read_text())
                for arm in findings.get("arms", []):
                    observed = arm.get("observed", "")
                    status = arm.get("status", "")
                    arm_type = arm.get("arm_type", "")

                    # Try to extract throughput from observed text
                    tp_match = _re.search(r"([\d.]+)\s*GB/s", observed)
                    throughput = float(tp_match.group(1)) if tp_match else None

                    # Match various p99 formats:
                    # "p99 latency: 884.6 us", "p99: 1129 us", "P99 latency: 875.5 us"
                    # "p99 latency improved from X us to Y us" (take Y)
                    p99_match = _re.search(
                        r"[Pp]99[^:]*?(?:to\s+)?([\d.]+)\s*us", observed
                    )
                    if not p99_match:
                        p99_match = _re.search(r"[Pp]99[^0-9]*([\d.]+)\s*us", observed)
                    p99_ms = float(p99_match.group(1)) / 1000.0 if p99_match else None

                    # Compute score if we have data
                    score = 0.0
                    if throughput and throughput > 0:
                        tp_component = min(1.0, throughput / 12.0)
                        lat_component = min(1.0, 0.4 / p99_ms) if p99_ms and p99_ms > 0 else 0.0
                        score = round(0.6 * tp_component + 0.4 * lat_component, 4)

                    entry = {
                        "combined_score": score,
                        "iteration": i,
                        "arm_type": arm_type,
                        "throughput_gbps": throughput,
                        "p99_latency_ms": p99_ms,
                        "build_succeeded": throughput is not None and throughput > 0,
                        "failure_type": "success" if score > 0 else "build_failure",
                        "error": "" if score > 0 else observed[:200],
                        "status": status,
                    }
                    f.write(json.dumps(entry) + "\n")
            except (json.JSONDecodeError, ValueError):
                continue


def _extract_shinkaevolve_scores(run_dir: Path):
    """Extract scores from ShinkaEvolve's SQLite DB into scores.jsonl."""
    import re as _re
    import sqlite3

    db_path = run_dir / "output" / "programs.sqlite"
    if not db_path.exists():
        return

    try:
        conn = sqlite3.connect(str(db_path))
        conn.row_factory = sqlite3.Row
        rows = conn.execute(
            "SELECT generation, combined_score, public_metrics FROM programs ORDER BY generation, timestamp"
        ).fetchall()
        conn.close()
    except Exception:
        return

    output_dir = run_dir / "output"
    eval_re = _re.compile(r"Score: ([\d.]+) \| Throughput: ([\d.]+) GB/s \| p99: (\d+)us \| CPU: ([\d.]+)%")

    scores_file = run_dir / "scores.jsonl"
    with open(scores_file, "w") as f:
        for r in rows:
            metrics = json.loads(r["public_metrics"]) if r["public_metrics"] else {}
            score = float(r["combined_score"] or 0.0)
            gen = r["generation"]

            throughput = metrics.get("throughput_gbps")
            p99 = metrics.get("p99_latency_ms")
            cpu = metrics.get("cpu_util_fraction")

            if throughput is None:
                log_out = output_dir / f"gen_{gen}" / "results" / "job_log.out"
                if log_out.exists():
                    m = eval_re.search(log_out.read_text())
                    if m:
                        throughput = float(m.group(2))
                        p99 = float(m.group(3)) / 1000.0
                        cpu = float(m.group(4)) / 100.0

            failure_type = "success" if score > 0 else "build_failure"
            error = ""
            if score == 0.0:
                log_out = output_dir / f"gen_{gen}" / "results" / "job_log.out"
                if log_out.exists():
                    text = log_out.read_text().strip()
                    if "Server failed" in text:
                        failure_type = "server_startup_failure"
                    elif "Benchmark failed" in text:
                        failure_type = "integrity_failure"
                    error = text[:200]

            entry = {
                "combined_score": round(score, 4),
                "iteration": gen,
                "throughput_gbps": throughput,
                "p99_latency_ms": p99,
                "cpu_util_fraction": cpu,
                "build_succeeded": score > 0,
                "data_integrity": failure_type != "integrity_failure",
                "failure_type": failure_type,
                "error": error,
            }
            f.write(json.dumps(entry) + "\n")


def _analyze_run(name: str, run_dir: Path, scores: list[float], log_file: Path):
    """Post-run analysis: diagnose failures, detect patterns, write analysis.json."""
    import re as _re

    analysis = {
        "framework": name,
        "total_evals": len(scores),
        "best_score": max(scores) if scores else 0.0,
        "beat_baseline": max(scores) > 0.22 if scores else False,
        "p2p_attempted": False,
        "p2p_compiled": False,
        "stagnation_ceiling": None,
        "primary_failure_mode": None,
        "build_errors": [],
        "diagnosis": "",
    }

    # Read log and scores.jsonl for error analysis
    log_text = ""
    try:
        log_text = log_file.read_text()
    except OSError:
        pass

    scores_entries = []
    scores_file = run_dir / "scores.jsonl"
    if scores_file.exists():
        for line in scores_file.read_text().splitlines():
            try:
                scores_entries.append(json.loads(line))
            except (json.JSONDecodeError, ValueError):
                pass

    # Detect P2P attempt — look for P2P function CALLS (not just definitions)
    p2p_call_patterns = [
        r"dma::create_spdk_dma_buffer_from_gpu_bar",
        r"dma::create_spdk_dma_buffer_from_gpu\(",
        r"let.*=.*create_spdk_dma_buffer_from_gpu_bar",
        r"GpuDirectBuffer::new",
        r"GpuDirectBuffer\s*\{",
    ]
    # Build errors mentioning P2P (means they tried to call it but failed)
    p2p_error_markers = [
        "cannot find function `create_spdk_dma_buffer_from_gpu",
        "cannot find function `create_gpu_dma_buffer",
    ]

    all_text = log_text
    for entry in scores_entries:
        all_text += " " + str(entry.get("error", ""))

    # Also scan candidate source files for P2P calls
    candidates_dir = run_dir / "candidates"
    if candidates_dir.exists():
        for gen_dir in sorted(candidates_dir.iterdir(), reverse=True):
            if not gen_dir.is_dir():
                continue
            for rs_file in gen_dir.glob("*.rs"):
                try:
                    all_text += " " + rs_file.read_text()
                except OSError:
                    pass
            break  # only check the latest generation

    # For nous: scan .nous/ bundle yamls and findings for P2P evidence
    if name == "nous":
        repo_root = EXPERIMENT_DIR.parents[1]
        for bundle_file in repo_root.glob(".nous/p2p-evolve-*/runs/iter-*/bundle.yaml"):
            try:
                all_text += " " + bundle_file.read_text()
            except OSError:
                pass

    p2p_called = any(_re.search(p, all_text) for p in p2p_call_patterns)
    p2p_in_errors = any(m in all_text for m in p2p_error_markers)
    analysis["p2p_attempted"] = p2p_called or p2p_in_errors

    # P2P compiled = check if the BEST candidate's source actually contains P2P calls
    # Read the actual candidate files to verify (no score threshold heuristics)
    if analysis["p2p_attempted"]:
        best_candidate_has_p2p = False

        # Check candidates (coding_agent saves gen_N/ dirs)
        if candidates_dir.exists():
            for gen_dir in sorted(candidates_dir.iterdir(), reverse=True):
                if not gen_dir.is_dir():
                    continue
                for rs_file in gen_dir.glob("*.rs"):
                    content = rs_file.read_text()
                    # Look for P2P calls in the actual source (not definitions)
                    if _re.search(r"(?<!pub fn )create_spdk_dma_buffer_from_gpu_bar\(", content):
                        best_candidate_has_p2p = True
                        break
                    if "GpuDirectBuffer::new(" in content or "GpuDirectBuffer {" in content:
                        best_candidate_has_p2p = True
                        break
                if best_candidate_has_p2p:
                    break

        # Also check the best candidate from GEPA/SkyDiscover (candidates.json or output dir)
        if not best_candidate_has_p2p:
            for candidates_json in run_dir.glob("candidates.json"):
                try:
                    cdata = json.loads(candidates_json.read_text())
                    # GEPA stores best candidate text
                    for key, val in (cdata if isinstance(cdata, dict) else {}).items():
                        if isinstance(val, str) and _re.search(
                            r"(?<!pub fn )create_spdk_dma_buffer_from_gpu_bar\(", val
                        ):
                            best_candidate_has_p2p = True
                            break
                except (json.JSONDecodeError, OSError):
                    pass

        # For nous: no candidate files saved, but if bundles describe P2P code
        # changes and the code ran (build_succeeded with throughput > 0), it compiled
        if not best_candidate_has_p2p and name == "nous":
            for entry in scores_entries:
                if entry.get("build_succeeded") and entry.get("throughput_gbps", 0) > 0:
                    best_candidate_has_p2p = True
                    break

        # Count builds before checking p2p_compiled
        build_successes = [e for e in scores_entries if e.get("build_succeeded", True)]

        # P2P compiled = candidate has P2P calls AND it built successfully
        analysis["p2p_compiled"] = (
            best_candidate_has_p2p
            and len(build_successes) > 0
        )

    # Diagnose build failures
    build_failures = [e for e in scores_entries if not e.get("build_succeeded", True)]
    build_successes = [e for e in scores_entries if e.get("build_succeeded", True)]
    analysis["build_fail_count"] = len(build_failures)
    analysis["build_success_count"] = len(build_successes)

    # Extract unique build error patterns
    error_patterns = set()
    for e in build_failures:
        err = str(e.get("error", ""))
        # Extract rust error codes
        for m in _re.finditer(r"error\[E\d+\]: (.+?)(?:\\n|\n|$)", err):
            error_patterns.add(m.group(1)[:80])
        # FFI type issues
        if "c_int" in err or "i32" in err or "u32" in err:
            error_patterns.add("FFI type mismatch (c_int vs u32)")
        if "cannot find" in err:
            for m in _re.finditer(r"cannot find (?:function|struct|type) `(\w+)`", err):
                error_patterns.add(f"Cannot find: {m.group(1)}")
    analysis["build_errors"] = list(error_patterns)[:10]

    # Stagnation analysis
    p2p_compiled = analysis.get("p2p_compiled", False)
    p2p_attempted = analysis.get("p2p_attempted", False)
    if scores:
        best = max(scores)
        if best <= 0.22:
            analysis["stagnation_ceiling"] = "baseline"
            if len(build_failures) > len(scores) * 0.4:
                analysis["primary_failure_mode"] = "build_failures"
                analysis["diagnosis"] = (
                    f"Failed to compile P2P code ({len(build_failures)}/{len(scores)} build failures). "
                    f"When it compiled, score stayed at baseline."
                )
            elif p2p_compiled:
                analysis["primary_failure_mode"] = "p2p_slower"
                analysis["diagnosis"] = (
                    "P2P compiled and ran but performed at or below baseline. "
                    "Hardware limitation (GPU L2 cache coherence with external PCIe DMA, "
                    "BAR1 VA not recognized as pinned memory by CUDA)."
                )
            elif p2p_attempted:
                analysis["primary_failure_mode"] = "p2p_wiring"
                analysis["diagnosis"] = (
                    "Generated P2P structures but didn't successfully wire them "
                    "into the active transfer pipeline."
                )
            else:
                analysis["primary_failure_mode"] = "no_improvement"
                analysis["diagnosis"] = (
                    "Code compiled but no improvement over baseline. "
                    "Did not attempt P2P path."
                )
        elif best < 0.42:
            analysis["stagnation_ceiling"] = "mid_range"
            analysis["primary_failure_mode"] = "local_optimum"
            if p2p_compiled:
                analysis["diagnosis"] = (
                    f"P2P compiled and improved to {best:.4f} ({best/0.20*100 - 100:.0f}% over baseline). "
                    "Plateaued due to staging copy overhead or pipeline sync costs."
                )
            else:
                analysis["diagnosis"] = (
                    f"Improved to {best:.4f} via host-bounce optimizations (QD/sync tuning). "
                    "Did not successfully implement P2P."
                )
        else:
            analysis["stagnation_ceiling"] = "hardware_limit"
            analysis["primary_failure_mode"] = "near_ceiling"
            analysis["diagnosis"] = (
                f"Achieved {best:.4f} — near hardware limit. "
                "Further gains require P2P (bypass host DRAM) or multi-drive."
            )

    # Write analysis
    (run_dir / "analysis.json").write_text(json.dumps(analysis, indent=2))

    # Print summary
    print(f"  [ANALYSIS] {analysis['diagnosis']}")
    if analysis["build_errors"]:
        print(f"  [ERRORS] {'; '.join(analysis['build_errors'][:3])}")


def _parse_scores(run_dir: Path) -> list[float]:
    """Parse scores from framework output."""
    import re as _re
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
    if not scores:
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

    # Fallback: parse combined_score from run log (SkyDiscover/GEPA evaluator output)
    if not scores:
        for log_file in sorted(run_dir.glob("run-*.log")):
            try:
                text = log_file.read_text()
                for m in _re.finditer(r"combined_score=([\d.]+)", text):
                    s = float(m.group(1))
                    if s > 0:
                        scores.append(s)
            except OSError:
                pass

    # Also write scores.jsonl if we found scores but file didn't exist
    if scores and not scores_file.exists():
        with open(scores_file, "w") as f:
            for i, s in enumerate(scores):
                f.write(json.dumps({
                    "combined_score": round(s, 4),
                    "iteration": i,
                    "build_succeeded": True,
                    "failure_type": "success",
                    "error": "",
                }) + "\n")

    return scores


# --- Config/seed generation helpers ---

def _ensure_concatenated_seed() -> Path:
    """Build slim concatenated seed for SkyDiscover frameworks."""
    concat_path = EXPERIMENT_DIR / "initial_programs" / "concatenated_seed.rs.skydiscover"
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


def _ensure_nous_config(config_file: Path, repo_path: str = None):
    """Create Nous campaign config (always rewritten to include correct repo_path)."""
    config_file.parent.mkdir(parents=True, exist_ok=True)
    if repo_path is None:
        repo_path = str(REPO_ROOT)

    config_file.write_text(f"""name: p2p-evolution
repo_path: {repo_path}
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
    parser.add_argument(
        "--hints", action="store_true",
        help="Use hint configs (explicit P2P direction). Results go to results_hint/.",
    )
    args = parser.parse_args()

    # Switch configs and results dir based on --hints
    global RESULTS_DIR
    if args.hints:
        active_configs = CONFIGS_HINT_DIR
        RESULTS_DIR = RESULTS_HINT_DIR
    else:
        active_configs = CONFIGS_DIR

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    frameworks = args.frameworks.split(",") if args.frameworks else ALL_FRAMEWORKS

    mode_label = "WITH HINTS (P2P directed)" if args.hints else "NO HINTS (control)"
    print(f"{'='*70}")
    print(f"  P2P Evolution Experiment — {mode_label}")
    print(f"{'='*70}")
    print(f"  Frameworks: {frameworks}")
    print(f"  Iterations: {args.iterations}")
    print(f"  Results: {RESULTS_DIR}")
    print(f"  Configs: {active_configs}")
    print(f"  Evaluator: {EVALUATOR_PY}")
    print(f"  Scoring: 60% throughput + 40% latency")
    print(f"  Data PCI: {os.environ.get('CERTUS_DATA_PCI', '0000:62:00.0')}")
    print()

    summaries = []
    for name in frameworks:
        if name not in ALL_FRAMEWORKS:
            print(f"  WARNING: Unknown framework '{name}', skipping")
            continue
        summary = run_framework(name, args.iterations, hint_configs=active_configs if args.hints else None)
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

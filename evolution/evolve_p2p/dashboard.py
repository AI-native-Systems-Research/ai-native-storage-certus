#!/usr/bin/env python3
"""Evolve P2P Experiment Dashboard.

Streamlit app that reads local result files and shows:
1. Overview — summary table + top-line cards
2. Trajectories — score vs evaluation number per framework
3. Pareto — throughput vs latency scatter with Pareto frontier
4. Candidate Details — click into individual evaluations
5. Failure Analysis — stacked bars + error tables
6. Architecture Classification — what kind of change each candidate made

Run:
    streamlit run dashboard.py
"""
from __future__ import annotations

import difflib
import json
import os
import re
from pathlib import Path

import pandas as pd
import plotly.express as px
import plotly.graph_objects as go
import streamlit as st

_EXPERIMENT_DIR = Path(__file__).resolve().parent
_RESULTS_NO_HINT = _EXPERIMENT_DIR / "results"
_RESULTS_HINT = _EXPERIMENT_DIR / "results_hint"
INITIAL_PROGRAMS = Path(__file__).resolve().parent / "initial_programs"
WILD_TYPE_SCORE = 0.2026

# Scoring formula ceiling (used for fitness normalization — don't change mid-experiment)
SCORING_CEILING_GBPS = 12.0
LATENCY_TARGET_MS = 0.4

# Actual hardware ceilings (measured) — for dashboard display
SINGLE_DRIVE_GBPS = 5.92          # spdk_nvme_perf, single drive seq read, QD=64+
H2D_4MIB_GBPS = 16.8             # cudaMemcpy H2D, pinned, 4 MiB blocks
DRIVES_IN_CONFIG = int(os.environ.get("CERTUS_DRIVE_COUNT", "1"))
CLIENTS_IN_CONFIG = int(os.environ.get("CERTUS_CLIENT_COUNT", "1"))

# Actual ceiling for current config: min(NVMe aggregate, H2D bandwidth)
NVME_AGGREGATE_GBPS = SINGLE_DRIVE_GBPS * DRIVES_IN_CONFIG
ACTUAL_CEILING_GBPS = min(NVME_AGGREGATE_GBPS, H2D_4MIB_GBPS)

FRAMEWORK_COLORS = {
    "gepa_native": "#1f77b4",
    "adaevolve": "#ff7f0e",
    "evox": "#2ca02c",
    "openevolve": "#d62728",
    "shinkaevolve": "#9467bd",
    "ksearch": "#8c564b",
    "nous": "#e377c2",
    "autoscientists": "#bcbd22",
}

FAILURE_COLORS = {
    "success": "#2ca02c",
    "build_failure": "#d62728",
    "server_startup_failure": "#ff7f0e",
    "benchmark_timeout": "#9467bd",
    "integrity_failure": "#e377c2",
    "parse_failure": "#bcbd22",
    "crash": "#17becf",
    "other_failure": "#7f7f7f",
}


def split_concatenated(text: str) -> dict[str, str]:
    """Split a concatenated multi-file string on '// --- FILE: xxx ---' markers."""
    marker_re = re.compile(r"^//\s*---\s*FILE:\s*(\S+?)(?:\s*\(.*?\))?\s*---\s*$", re.MULTILINE)
    markers = list(marker_re.finditer(text))
    if not markers:
        return {"pipeline.rs": text}
    files = {}
    for i, m in enumerate(markers):
        filename = m.group(1)
        start = m.end()
        end = markers[i + 1].start() if i + 1 < len(markers) else len(text)
        content = text[start:end].strip("\n")
        files[filename] = content
    return files


@st.cache_data(ttl=10)
def load_all_scores(results_dir: str = "") -> pd.DataFrame:
    """Load scores.jsonl from all framework result dirs."""
    _dir = Path(results_dir) if results_dir else _RESULTS_NO_HINT
    rows = []
    if not _dir.exists():
        return pd.DataFrame()

    for fw_dir in sorted(_dir.iterdir()):
        if not fw_dir.is_dir() or fw_dir.name in ("summary", "random"):
            continue
        framework = fw_dir.name

        scores_file = fw_dir / "scores.jsonl"
        if scores_file.exists():
            for i, line in enumerate(scores_file.read_text().splitlines()):
                try:
                    entry = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                row = _entry_to_row(entry, framework, i)
                rows.append(row)

        # For SkyDiscover frameworks: parse the log for all evaluations
        # (captures both successes AND build failures in one pass)
        if not scores_file.exists():
            found_evals = False
            for log_file in sorted(fw_dir.glob("run-*.log")):
                eval_rows = _parse_skydiscover_log(log_file, framework)
                if eval_rows:
                    found_evals = True
                    rows.extend(eval_rows)

            # Frameworks that crashed before producing any evaluations:
            # synthesize a single row from summary.json so they appear in the dashboard
            if not found_evals:
                summary_file = fw_dir / "summary.json"
                if summary_file.exists():
                    try:
                        summary = json.loads(summary_file.read_text())
                        failure_type = "crash" if summary.get("returncode", 0) != 0 else "other_failure"
                        rows.append({
                            "framework": framework,
                            "eval_num": 0,
                            "score": 0.0,
                            "throughput_gbps": None,
                            "p99_latency_ms": None,
                            "p50_latency_ms": None,
                            "mean_latency_ms": None,
                            "cpu_util_fraction": None,
                            "build_succeeded": False,
                            "data_integrity": True,
                            "failure_type": failure_type,
                            "error": f"Framework crashed (exit {summary.get('returncode', '?')}, "
                                     f"{summary.get('wall_time_seconds', 0):.0f}s wall time)",
                        })
                    except (json.JSONDecodeError, ValueError):
                        pass

    if not rows:
        return pd.DataFrame()

    df = pd.DataFrame(rows)
    df["score"] = pd.to_numeric(df["score"], errors="coerce").fillna(0.0)
    return df


def _flatten_skydiscover_entry(entry: dict, idx: int) -> dict | None:
    """Extract metrics from SkyDiscover's nested iteration stats format."""
    child = entry.get("iteration_result", {}).get("child_program", {})
    metrics = child.get("metrics", {})
    if not metrics:
        # Try top-level best_program
        best = entry.get("global", {}).get("best_program", {})
        metrics = best.get("metrics", {})
    if not metrics or "combined_score" not in metrics:
        return None
    return {
        "combined_score": metrics.get("combined_score", 0.0),
        "iteration": entry.get("iteration", idx),
        "throughput_gbps": metrics.get("throughput_gbps"),
        "p99_latency_ms": metrics.get("p99_latency_ms"),
        "p50_latency_ms": metrics.get("p50_latency_ms"),
        "mean_latency_ms": metrics.get("mean_latency_ms"),
        "cpu_util_fraction": metrics.get("cpu_util_fraction"),
        "build_succeeded": metrics.get("build_succeeded", True),
        "data_integrity": metrics.get("data_integrity", True),
    }


def _parse_skydiscover_log(log_path, framework: str) -> list[dict]:
    """Parse SkyDiscover log file for all evaluations (successes + failures)."""
    rows = []
    eval_re = re.compile(
        r"\[evaluator\] Evaluated program (\S+) in ([\d.]+)s: (.+)"
    )
    try:
        text = log_path.read_text()
    except OSError:
        return rows

    for i, match in enumerate(eval_re.finditer(text)):
        prog_id = match.group(1)
        eval_time = float(match.group(2))
        rest = match.group(3)

        # Parse key=value pairs from the rest
        metrics = {}
        for kv in re.finditer(r"(\w+)=([\w.+-]+|True|False)", rest):
            k, v = kv.group(1), kv.group(2)
            if v == "True":
                metrics[k] = True
            elif v == "False":
                metrics[k] = False
            else:
                try:
                    metrics[k] = float(v)
                except ValueError:
                    metrics[k] = v

        # Check if it's an error line
        is_error = "error=" in rest or "error[E" in rest
        if is_error and "combined_score" not in metrics:
            metrics["combined_score"] = 0.0
            metrics["build_succeeded"] = False
            # Extract error text
            err_match = re.search(r"error=(.+)", rest)
            metrics["error"] = err_match.group(1)[:200] if err_match else "build_failure"

        score = metrics.get("combined_score", 0.0)
        build_ok = metrics.get("build_succeeded", score > 0)
        data_ok = metrics.get("data_integrity", True)
        error = metrics.get("error", "")

        if not build_ok:
            failure_type = "build_failure"
        elif not data_ok:
            failure_type = "integrity_failure"
        elif score > 0:
            failure_type = "success"
        else:
            failure_type = "other_failure"

        rows.append({
            "framework": framework,
            "eval_num": i,
            "score": score,
            "throughput_gbps": metrics.get("throughput_gbps"),
            "p99_latency_ms": metrics.get("p99_latency_ms"),
            "p50_latency_ms": metrics.get("p50_latency_ms"),
            "mean_latency_ms": metrics.get("mean_latency_ms"),
            "cpu_util_fraction": metrics.get("cpu_util_fraction"),
            "build_succeeded": build_ok,
            "data_integrity": data_ok,
            "failure_type": failure_type,
            "error": str(error)[:200] if error else "",
        })
    return rows


def _entry_to_row(entry: dict, framework: str, fallback_idx: int) -> dict:
    build_ok = entry.get("build_succeeded", True)
    data_ok = entry.get("data_integrity", True)
    score = entry.get("combined_score", 0.0)

    failure_type = entry.get("failure_type")
    if not failure_type:
        if not build_ok:
            failure_type = "build_failure"
        elif not data_ok:
            failure_type = "integrity_failure"
        elif score and score > 0:
            failure_type = "success"
        else:
            failure_type = "other_failure"

    return {
        "framework": framework,
        "eval_num": entry.get("iteration", fallback_idx),
        "score": score,
        "throughput_gbps": entry.get("throughput_gbps"),
        "p99_latency_ms": entry.get("p99_latency_ms"),
        "p50_latency_ms": entry.get("p50_latency_ms"),
        "mean_latency_ms": entry.get("mean_latency_ms"),
        "cpu_util_fraction": entry.get("cpu_util_fraction"),
        "build_succeeded": build_ok,
        "data_integrity": data_ok,
        "failure_type": failure_type,
        "error": entry.get("error", ""),
    }


def compute_best_so_far(df: pd.DataFrame) -> pd.DataFrame:
    if df.empty:
        return df
    df = df.sort_values(["framework", "eval_num"]).copy()
    df["best_so_far"] = df.groupby("framework")["score"].cummax()
    return df


def classify_architecture(candidate_files: dict[str, str], wild_type_files: dict[str, str]) -> dict:
    """Heuristic classification of what kind of change a candidate made."""
    signals = {
        "new_imports": False,
        "new_functions": False,
        "changed_constants_only": True,
        "gpu_direct_symbols": False,
        "pipeline_restructure": False,
        "ring_size_change": False,
    }

    gpu_direct_markers = [
        "dma::create_spdk_dma_buffer_from_gpu_bar",
        "create_spdk_dma_buffer_from_gpu_bar(",
        "GpuDirectBuffer::new",
        "GpuDirectBuffer {",
    ]
    pipeline_markers = [
        "async_pipeline", "multi_stream", "overlap", "double_buffer",
        "concurrent", "parallel_read", "batch_dma",
    ]

    total_added = 0
    total_removed = 0

    for filename in candidate_files:
        candidate = candidate_files[filename]
        wild = wild_type_files.get(filename, "")

        diff_lines = list(difflib.unified_diff(
            wild.splitlines(), candidate.splitlines(), lineterm=""
        ))

        added = [l[1:] for l in diff_lines if l.startswith("+") and not l.startswith("+++")]
        removed = [l[1:] for l in diff_lines if l.startswith("-") and not l.startswith("---")]
        total_added += len(added)
        total_removed += len(removed)

        added_text = "\n".join(added)

        if any(m in added_text for m in gpu_direct_markers):
            signals["gpu_direct_symbols"] = True
            signals["changed_constants_only"] = False

        if any(m in added_text for m in pipeline_markers):
            signals["pipeline_restructure"] = True
            signals["changed_constants_only"] = False

        if re.search(r"^(use |extern crate )", added_text, re.MULTILINE):
            signals["new_imports"] = True
            signals["changed_constants_only"] = False

        if re.search(r"^pub (fn|async fn) ", added_text, re.MULTILINE):
            signals["new_functions"] = True
            signals["changed_constants_only"] = False

        if re.search(r"PIPELINE_RING_SIZE", added_text):
            signals["ring_size_change"] = True

        # If anything beyond numeric constants changed
        non_const_changes = [l for l in added if not re.match(r"^\s*(pub )?(const|let|static)\s+\w+.*=\s*\d+", l)]
        if len(non_const_changes) > 3:
            signals["changed_constants_only"] = False

    # Classify
    if signals["gpu_direct_symbols"]:
        category = "path_change"
    elif signals["pipeline_restructure"] or (signals["new_functions"] and total_added > 20):
        category = "pipeline_restructure"
    elif signals["changed_constants_only"] and total_added < 10:
        category = "knob_tuning"
    elif total_added > 50 or total_removed > 30:
        category = "hybrid"
    else:
        category = "knob_tuning"

    return {
        "category": category,
        "signals": signals,
        "lines_added": total_added,
        "lines_removed": total_removed,
    }


@st.cache_data
def load_wild_type() -> dict[str, str]:
    """Load wild-type source files."""
    files = {}
    if INITIAL_PROGRAMS.exists():
        for f in INITIAL_PROGRAMS.iterdir():
            if f.suffix == ".rs":
                files[f.name] = f.read_text()
    return files


# --- Page config ---
st.set_page_config(page_title="SSD-to-GPU Evolution", page_icon="🧬", layout="wide")

# Sidebar: select experiment mode
with st.sidebar:
    st.header("Experiment")
    experiment_mode = st.radio(
        "Results source",
        ["Optimize data path (no hints)", "Implement P2P (with hints)"],
        index=0,
    )
    if experiment_mode == "Implement P2P (with hints)":
        RESULTS_DIR = _RESULTS_HINT
        st.caption("Frameworks given explicit P2P direction + FFI signatures")
    else:
        RESULTS_DIR = _RESULTS_NO_HINT
        st.caption("Frameworks discover optimizations independently")

# Title changes based on mode
if experiment_mode == "Implement P2P (with hints)":
    st.title("Implement P2P — Maximize Cold Lookup Throughput")
else:
    st.title("Optimize Data Transfer Path — Maximize Cold Lookup Throughput")

df = load_all_scores(str(RESULTS_DIR))

if df.empty:
    st.warning(
        "No results found. Run the experiment first:\n\n"
        "```\ncd evolution/evolve_p2p\n"
        "python run_experiment.py --frameworks gepa_native,random --iterations 10\n```"
    )
    st.stop()

df = compute_best_so_far(df)

# --- Tabs ---
tab_overview, tab_findings, tab_trajectory, tab_pareto, tab_details, tab_failures, tab_arch = st.tabs(
    ["Overview", "Findings", "Trajectories", "Pareto", "Candidate Details", "Failure Analysis", "Architecture"]
)

# ============================================================
# TAB: Key Findings
# ============================================================
with tab_findings:
    st.subheader("Key Findings")

    if experiment_mode == "Implement P2P (with hints)":
        st.markdown("""
**Baseline:** 2.4 GB/s, score 0.20 (1 queue × QD16, NVMe → DRAM → GPU host-bounce path)

---

#### What worked

| Framework | Score | Throughput | Approach |
|-----------|-------|-----------|----------|
| **autoscientists** | 0.3945 | 4.65 GB/s | P2P failed at runtime → fell back to host-bounce (QD64, multi-object pipeline, overlapped H2D) |
| **coding_agent** | 0.3431 | 3.97 GB/s | **P2P implemented successfully** — GDRCopy BAR1 staging ring + D2D copy |
| **gepa_native** | 0.3314 | 3.87 GB/s | Host-bounce only (QD/sync tuning). Never called P2P functions. |

---

#### P2P implementation: coding_agent

```
NVMe → GPU BAR1 staging ring (64 slots via cudaMalloc + GDRCopy) → cudaMemcpyAsync D2D → final gpu_dst
```

- Pre-allocates 64 GPU memory slots, maps into BAR1 via GDRCopy, registers with SPDK
- NVMe controller DMAs directly into GPU BAR1 memory (single PCIe hop)
- D2D copy at GPU internal bandwidth (~600 GB/s for 128KiB) — effectively zero-cost
- Host DRAM completely eliminated from data path
- Iteratively optimized: removed DRAM backfill, lazy stream sync, thread-partitioned ring slots

---

#### The nous vs coding_agent mystery

Both used the **same architecture** (GDRCopy BAR1 + D2D copy). Coding_agent got 3.97 GB/s. Nous got 0.01 GB/s (140x slower).

Nous reported "GPU L2 cache coherence — external PCIe DMA doesn't invalidate L2." But coding_agent's code uses the same `dev_ptr` and works fine.

Likely cause: implementation timing. Coding_agent's sliding-window has natural latency between NVMe write and D2D read (~22µs per chunk), enough for L2 eviction. Nous may have issued D2D too soon, or used a different buffer reference.

**Takeaway:** Architectural reasoning alone isn't sufficient. Success depends on subtle code-level details that hypothesis-driven agents miss but iterative optimizers stumble into.

---

#### Failure modes

| Mode | Frameworks | Description |
|------|-----------|-------------|
| **Success** | coding_agent | Iterative optimization found working P2P implementation |
| **Runtime failure** | autoscientists | P2P compiled but GDRCopy rc=22 on IPC memory; abandoned P2P |
| **Implementation failure** | nous | Same architecture as coding_agent but timing/buffering caused 140x regression |
| **Coordination failure** | gepa_native | Reproduced P2P definitions in dma.rs, never wired into pipeline.rs |
| **No attempt** | ksearch, openevolve | Stuck at baseline with conservative mutations |

---

#### Key takeaways

1. **Only 1 of 6 frameworks implemented working P2P.** Explicit hints fixed discovery but not coordination or implementation.
2. **P2P didn't win on throughput for single-drive.** AutoScientists' host-bounce (4.65 GB/s) beat coding_agent's P2P (3.97 GB/s). P2P advantage is latency (1.11ms vs ~1.2ms p99) and multi-drive scalability.
3. **Evolutionary frameworks cannot implement architectural changes** — with or without hints. They ignore direction or can't coordinate multi-file mutations.
4. **Same design, opposite results** (nous vs coding_agent) shows implementation details > architectural correctness for systems optimization.
5. **Iterative hill-climbing beat hypothesis-driven** for this task. Nous reasoned correctly but failed to implement. Coding_agent tried, measured, adjusted — and got it right.
""")
    else:
        st.markdown("""
**Baseline:** 2.4 GB/s, score 0.20 (1 queue × QD16, NVMe → DRAM → GPU host-bounce path)

---

#### What worked

| Framework | Score | Throughput | Approach |
|-----------|-------|-----------|----------|
| **nous** | 0.4808 | 5.96 GB/s | 4 queues × QD32, 4 CUDA streams, removed mid-pipeline sync |
| **coding_agent** | 0.3891 | 4.59 GB/s | Multi-object interleaving, QD36-38, 4 CUDA streams |
| **autoscientists** | 0.3368 | 3.94 GB/s | Removed periodic GPU sync, 2 queues QD32 |

---

#### No framework discovered P2P independently

All frameworks had access to dma.rs (containing P2P functions) via their seed or file scope. None independently identified P2P as an optimization:

- **Agentic frameworks** (nous, autoscientists, coding_agent): Focused on hot-path tuning in pipeline.rs/lib.rs. Never explored dma.rs deeply enough to identify P2P as a viable alternative architecture.
- **K-Search**: World model correctly identified GPUDirect as highest-value action (rated 8/10), attempted it across 5 rounds, but used wrong function name in every attempt (`create_gpu_dma_buffer` instead of `create_spdk_dma_buffer_from_gpu_bar`).
- **OpenEvolve**: Attempted P2P once with truncated function name (`create_spdk_dma_buffer_from_gpu` — missing `_bar`). Failed to compile.
- **Others** (adaevolve, evox, shinkaevolve, gepa_native): Made conservative parameter mutations only.

---

#### Why nous dominated

Nous's hypothesis-driven approach with ablation arms systematically explored the parameter space:
- Iter 1: Discovered that removing mid-pipeline sync + deeper QD gives 3.93 GB/s
- Iter 2: Tested thread parallelism — ablation (QD64 without extra threads) beat main hypothesis
- Iter 3: Combined 4 queues × QD32 + 4 CUDA streams → **5.96 GB/s** (near drive ceiling)

Key: ablation arms isolated individual variables. The ablation of iter-2 proved deeper queues > more threads, informing iter-3's winning config.

---

#### Failure modes (no-hint)

| Mode | Frameworks | Description |
|------|-----------|-------------|
| **Discovery failure** | nous, autoscientists, coding_agent | Never explored dma.rs; focused on hot-path tuning |
| **Implementation failure** | ksearch, openevolve | Identified P2P correctly but couldn't produce type-correct Rust (wrong function names) |
| **No attempt** | adaevolve, evox, shinkaevolve, gepa_native | Conservative mutations only; P2P in seed was ignored |

---

#### Key takeaways

1. **Deep NVMe pipelining is the #1 optimization** for single-drive: 1 queue × QD16 → 4 queues × QD32 gives 2.5x throughput with zero architectural change.
2. **No framework discovered P2P without explicit direction**, even with nvidia-peermem/gdrdrv mentioned in hardware specs and `--features p2p` in the build command.
3. **Agentic frameworks vastly outperform evolutionary** for multi-file optimization: nous (5.96 GB/s) vs adaevolve (2.44 GB/s).
4. **LLM recall failure on function names** blocked K-Search and OpenEvolve despite correct architectural reasoning.
5. **Local optimum traps** are real: coding_agent stopped at 2 queues × QD38 (4.59 GB/s). Nous's ablation approach found 4 queues × QD32 (5.96 GB/s) by testing structural alternatives.
""")


# ============================================================
# TAB 1: Overview
# ============================================================
with tab_overview:
    frameworks = sorted(
        df["framework"].unique().tolist(),
        key=lambda fw: df[df["framework"] == fw]["score"].max(),
        reverse=True,
    )

    col1, col2, col3, col4 = st.columns(4)
    best_row = df.loc[df["score"].idxmax()]
    total_evals = len(df)
    build_fails = int((df["failure_type"] == "build_failure").sum())
    integrity_fails = int((df["failure_type"] == "integrity_failure").sum())

    best_throughput = df["throughput_gbps"].max() if df["throughput_gbps"].notna().any() else 0.0
    wild_type_throughput = 2.39

    col1.metric("Wild-type score", f"{WILD_TYPE_SCORE:.4f}")
    col2.metric("Best score", f"{best_row['score']:.4f}",
                delta=f"+{best_row['score'] - WILD_TYPE_SCORE:.4f}")
    col3.metric("Best throughput", f"{best_throughput:.2f} GB/s" if best_throughput else "—",
                delta=f"+{best_throughput - wild_type_throughput:.2f}" if best_throughput else None)
    col4.metric("Actual ceiling", f"{ACTUAL_CEILING_GBPS:.1f} GB/s")

    col5, col6, col7, col8 = st.columns(4)
    col5.metric("Best framework", best_row["framework"])
    col6.metric("Total evaluations", total_evals)
    col7.metric("Build failures", build_fails)
    col8.metric("Frameworks tested", len(frameworks))

    st.markdown("---")

    summary_rows = []
    for fw in frameworks:
        fw_df = df[df["framework"] == fw]
        best = fw_df["score"].max()
        best_idx = fw_df["score"].idxmax()
        best_iter = fw_df.loc[best_idx, "eval_num"]
        throughput = fw_df["throughput_gbps"].max() if fw_df["throughput_gbps"].notna().any() else None
        p99 = fw_df.loc[best_idx, "p99_latency_ms"] if pd.notna(fw_df.loc[best_idx, "p99_latency_ms"]) else None
        n_build = int((fw_df["failure_type"] == "build_failure").sum())
        n_integ = int((fw_df["failure_type"] == "integrity_failure").sum())

        # Load wall time and cost from summary.json / cost.json
        fw_summary_file = RESULTS_DIR / fw / "summary.json"
        fw_cost_file = RESULTS_DIR / fw / "cost.json"
        wall_time = None
        cost_usd = None
        if fw_summary_file.exists():
            try:
                fw_summary = json.loads(fw_summary_file.read_text())
                wall_time = fw_summary.get("wall_time_seconds")
            except (json.JSONDecodeError, ValueError):
                pass
        if fw_cost_file.exists():
            try:
                cost_data = json.loads(fw_cost_file.read_text())
                cost_usd = cost_data.get("total_cost_usd")
            except (json.JSONDecodeError, ValueError):
                pass

        # P2P status from analysis.json
        p2p_status = "—"
        _analysis_path = RESULTS_DIR / fw / "analysis.json"
        if _analysis_path.exists():
            try:
                _a = json.loads(_analysis_path.read_text())
                if _a.get("p2p_compiled") and _a.get("beat_baseline"):
                    p2p_status = "P2P ✓ (faster)"
                elif _a.get("p2p_compiled"):
                    p2p_status = "P2P ✓ (slower)"
                elif _a.get("p2p_attempted"):
                    p2p_status = "Attempted"
                else:
                    p2p_status = "No"
            except (json.JSONDecodeError, OSError):
                pass

        summary_rows.append({
            "Framework": fw,
            "Best Score": f"{best:.4f}",
            "Δ baseline": f"+{best - WILD_TYPE_SCORE:.4f}" if best > WILD_TYPE_SCORE else f"{best - WILD_TYPE_SCORE:.4f}",
            "Throughput": f"{throughput:.2f}" if throughput else "—",
            "p99 (ms)": f"{p99:.3f}" if p99 else "—",
            "P2P": p2p_status,
            "Build Fails": n_build,
            "Evals": len(fw_df),
            "Wall Time": f"{wall_time/60:.1f}m" if wall_time else "—",
            "Cost": f"${cost_usd:.2f}" if cost_usd else "—",
        })

    st.dataframe(pd.DataFrame(summary_rows), use_container_width=True, hide_index=True)

# ============================================================
# TAB 2: Trajectories
# ============================================================
with tab_trajectory:
    st.subheader("Score Trajectory (best-so-far + raw scores)")

    fig = go.Figure()
    for fw in sorted(df["framework"].unique()):
        fw_df = df[df["framework"] == fw].sort_values("eval_num")
        color = FRAMEWORK_COLORS.get(fw, "#333333")

        # For frameworks with multiple arms per iteration, take best score per iteration
        if "iteration" in fw_df.columns:
            plot_df = fw_df.groupby("eval_num")["score"].max().reset_index()
        else:
            plot_df = fw_df[["eval_num", "score"]]

        fig.add_trace(go.Scatter(
            x=plot_df["eval_num"], y=plot_df["score"],
            mode="lines+markers", line=dict(width=2, color=color),
            marker=dict(size=5, color=color),
            name=fw, legendgroup=fw,
        ))

    fig.add_hline(y=WILD_TYPE_SCORE, line_dash="dash", line_color="red",
                  annotation_text="Wild-type baseline")
    fig.update_layout(
        xaxis_title="Evaluation Number", yaxis_title="Score",
        height=500, legend=dict(orientation="h", yanchor="bottom", y=1.02),
    )
    st.plotly_chart(fig, use_container_width=True)

    # Stagnation analysis — data-driven grouping by score range
    st.markdown("#### Stagnation Analysis")

    # Group frameworks by their peak score range
    ceiling_groups = {"near_baseline": [], "mid_range": [], "high": []}
    for fw in sorted(df["framework"].unique()):
        fw_df = df[df["framework"] == fw]
        peak = fw_df["score"].max()
        n_evals = len(fw_df)
        n_build_fail = (fw_df["build_succeeded"] == False).sum() if "build_succeeded" in fw_df.columns else 0
        build_fail_pct = n_build_fail / n_evals * 100 if n_evals > 0 else 0

        entry = {"fw": fw, "peak": peak, "evals": n_evals, "build_fail_pct": build_fail_pct}
        if peak <= WILD_TYPE_SCORE + 0.02:
            ceiling_groups["near_baseline"].append(entry)
        elif peak < 0.42:
            ceiling_groups["mid_range"].append(entry)
        else:
            ceiling_groups["high"].append(entry)

    if ceiling_groups["high"]:
        fws = ", ".join(f"**{e['fw']}** ({e['peak']:.4f})" for e in ceiling_groups["high"])
        st.markdown(f"**Near hardware ceiling** (score >0.42): {fws}")
        st.markdown("  - Hitting drive bandwidth limit (~5.9 GB/s). Further gains require P2P or multi-drive.")

    if ceiling_groups["mid_range"]:
        fws = ", ".join(f"**{e['fw']}** ({e['peak']:.4f})" for e in ceiling_groups["mid_range"])
        st.markdown(f"**Mid-range plateau** (score 0.22-0.42): {fws}")
        st.markdown("  - Found some improvements but stuck in local optimum. Likely limited by queue architecture or missing structural change.")

    if ceiling_groups["near_baseline"]:
        fws_info = []
        for e in ceiling_groups["near_baseline"]:
            fail_note = f", {e['build_fail_pct']:.0f}% build failures" if e["build_fail_pct"] > 20 else ""
            fws_info.append(f"**{e['fw']}** ({e['peak']:.4f}{fail_note})")
        st.markdown(f"**Stuck at baseline** (score ≤{WILD_TYPE_SCORE+0.02:.4f}): {', '.join(fws_info)}")
        st.markdown("  - Couldn't produce compiling improvements, or compiled code was functionally identical to baseline.")

# ============================================================
# TAB 3: Pareto
# ============================================================
with tab_pareto:
    st.subheader("Throughput vs p99 Latency")

    pareto_df = df[df["throughput_gbps"].notna() & df["p99_latency_ms"].notna()].copy()

    if pareto_df.empty:
        st.info("No throughput/latency data yet. Waiting for evals with full metrics.")
    else:
        fig = px.scatter(
            pareto_df, x="p99_latency_ms", y="throughput_gbps",
            color="framework", color_discrete_map=FRAMEWORK_COLORS,
            hover_data=["eval_num", "score"],
            labels={
                "p99_latency_ms": "p99 Latency (ms) ← lower is better",
                "throughput_gbps": "Throughput (GB/s) ↑ higher is better",
            },
        )

        # Pareto frontier
        sorted_p = pareto_df.sort_values("p99_latency_ms")
        frontier = []
        max_tp = -1.0
        for _, row in sorted_p.iterrows():
            if row["throughput_gbps"] > max_tp:
                max_tp = row["throughput_gbps"]
                frontier.append(row)
        if frontier:
            fdf = pd.DataFrame(frontier)
            fig.add_trace(go.Scatter(
                x=fdf["p99_latency_ms"], y=fdf["throughput_gbps"],
                mode="lines", line=dict(width=2, color="black", dash="dot"),
                name="Pareto frontier",
            ))

        # Wild-type marker
        fig.add_trace(go.Scatter(
            x=[1.753], y=[2.39], mode="markers",
            marker=dict(size=14, symbol="diamond", color="red", line=dict(width=2, color="black")),
            name="Wild-type (1d/1c)",
        ))

        # Actual hardware ceiling line
        fig.add_hline(y=ACTUAL_CEILING_GBPS, line_dash="dash", line_color="green",
                      annotation_text=f"HW ceiling ({DRIVES_IN_CONFIG}d): {ACTUAL_CEILING_GBPS:.1f} GB/s")

        fig.update_layout(height=500)
        fig.update_xaxes(autorange="reversed")
        st.plotly_chart(fig, use_container_width=True)

# ============================================================
# TAB 4: Candidate Details
# ============================================================
with tab_details:
    st.subheader("Candidate Details")

    col_fw, col_iter = st.columns(2)
    sel_framework = col_fw.selectbox("Framework", sorted(df["framework"].unique()))
    fw_df = df[df["framework"] == sel_framework].sort_values("eval_num").reset_index(drop=True)

    if fw_df.empty:
        st.info("No data for this framework.")
    else:
        best_local_idx = int(fw_df["score"].idxmax())
        sel_eval = col_iter.selectbox(
            "Evaluation", fw_df["eval_num"].tolist(), index=best_local_idx,
            format_func=lambda x: f"Eval {x} — {fw_df[fw_df['eval_num'] == x]['score'].values[0]:.4f}",
        )

        candidate = fw_df[fw_df["eval_num"] == sel_eval].iloc[0]

        mc1, mc2, mc3, mc4 = st.columns(4)
        mc1.metric("Score", f"{candidate['score']:.4f}")
        mc2.metric("Throughput", f"{candidate['throughput_gbps']:.2f} GB/s" if pd.notna(candidate.get("throughput_gbps")) else "—")
        mc3.metric("p99 Latency", f"{candidate['p99_latency_ms']:.3f} ms" if pd.notna(candidate.get("p99_latency_ms")) else "—")
        mc4.metric("CPU Util", f"{candidate['cpu_util_fraction']:.1%}" if pd.notna(candidate.get("cpu_util_fraction")) else "—")

        mc5, mc6, mc7, mc8 = st.columns(4)
        mc5.metric("p50 Latency", f"{candidate['p50_latency_ms']:.3f} ms" if pd.notna(candidate.get("p50_latency_ms")) else "—")
        mc6.metric("Build", "PASS" if candidate.get("build_succeeded") else "FAIL")
        mc7.metric("Integrity", "PASS" if candidate.get("data_integrity") else "FAIL")
        mc8.metric("Failure Type", candidate.get("failure_type", "—"))

        if candidate.get("error") and candidate.get("failure_type") != "success":
            st.error(f"**Error:** {candidate['error']}")

        # Candidate source files
        st.markdown("### Source")
        candidates_dir = RESULTS_DIR / sel_framework / "candidates"
        best_dir = RESULTS_DIR / sel_framework / "best"
        output_dir = RESULTS_DIR / sel_framework / "output"

        candidate_content = None

        # Method 1: Per-gen candidate dirs (GEPA, K-Search: candidates/gen_N/main.rs)
        if candidates_dir.exists():
            gen_dir = candidates_dir / f"gen_{int(sel_eval)}"
            main_rs = gen_dir / "main.rs"
            if main_rs.exists():
                candidate_content = split_concatenated(main_rs.read_text())
            elif gen_dir.exists():
                # Individual .rs files (coding_agent format)
                found = {}
                for f in gen_dir.iterdir():
                    if f.suffix == ".rs":
                        found[f.name] = f.read_text()
                if found:
                    candidate_content = found
            else:
                # Legacy GEPA format: candidates/001_pipeline.rs
                eval_idx = int(sel_eval) + 1
                found = []
                for suffix in ["pipeline.rs", "lib.rs", "dma.rs"]:
                    p = candidates_dir / f"{eval_idx:03d}_{suffix}"
                    if p.exists():
                        found.append((suffix, p.read_text()))
                if found:
                    candidate_content = dict(found)

        # Method 2: ShinkaEvolve per-gen files (output/gen_N/main.rs)
        if candidate_content is None and output_dir.exists():
            gen_dir = output_dir / f"gen_{int(sel_eval)}"
            main_rs = gen_dir / "main.rs"
            if main_rs.exists():
                candidate_content = split_concatenated(main_rs.read_text())

        # Method 3: SkyDiscover best program (output/best/best_program.skydiscover)
        if candidate_content is None and output_dir.exists():
            best_prog = output_dir / "best" / "best_program.skydiscover"
            if best_prog.exists():
                candidate_content = split_concatenated(best_prog.read_text())

        # Method 4: GEPA best/ dir
        if candidate_content is None and best_dir.exists():
            found = {}
            for f in sorted(best_dir.iterdir()):
                if f.suffix == ".rs":
                    found[f.name] = f.read_text()
            if found:
                candidate_content = found

        # Method 5: Agentic frameworks (nous, autoscientists) — read from repo working tree
        if candidate_content is None and sel_framework in ("nous", "autoscientists"):
            repo_root = RESULTS_DIR.parents[2]
            target_files = {
                "pipeline.rs": repo_root / "components" / "dispatcher" / "src" / "pipeline.rs",
                "lib.rs": repo_root / "components" / "dispatcher" / "src" / "lib.rs",
                "dma.rs": repo_root / "components" / "gpu-services" / "src" / "dma.rs",
            }
            found = {}
            for name, path in target_files.items():
                if path.exists():
                    found[name] = path.read_text()
            if found:
                candidate_content = found

        # Display diff
        if candidate_content:
            wild_type = load_wild_type()
            for name, content in sorted(candidate_content.items()):
                wt = wild_type.get(name, "")
                diff = difflib.unified_diff(
                    wt.splitlines(), content.splitlines(),
                    fromfile=f"wild-type/{name}", tofile=f"candidate/{name}", lineterm="",
                )
                diff_text = "\n".join(diff)
                with st.expander(f"{name} ({'+' if diff_text else '='}{len(content.splitlines())} lines)"):
                    if diff_text:
                        st.code(diff_text, language="diff")
                    else:
                        st.info("No changes from wild-type")
        else:
            st.info("No candidate source files available for this evaluation.")

# ============================================================
# TAB 5: Failure Analysis
# ============================================================
with tab_failures:
    st.subheader("Failure Analysis")

    # Stacked bar chart per framework
    failure_counts = df.groupby(["framework", "failure_type"]).size().reset_index(name="count")

    if failure_counts.empty:
        st.info("No data yet.")
    else:
        fig = px.bar(
            failure_counts, x="framework", y="count", color="failure_type",
            color_discrete_map=FAILURE_COLORS,
            labels={"count": "Evaluations", "framework": "Framework", "failure_type": "Outcome"},
            barmode="stack",
        )
        fig.update_layout(height=400)
        st.plotly_chart(fig, use_container_width=True)

        # Success rate table
        st.markdown("### Success Rate")
        rate_rows = []
        for fw in sorted(df["framework"].unique()):
            fw_df = df[df["framework"] == fw]
            total = len(fw_df)
            successes = int((fw_df["failure_type"] == "success").sum())
            builds = int((fw_df["failure_type"] == "build_failure").sum())
            startups = int((fw_df["failure_type"] == "server_startup_failure").sum())
            timeouts = int((fw_df["failure_type"] == "benchmark_timeout").sum())
            integrity = int((fw_df["failure_type"] == "integrity_failure").sum())
            parse = int((fw_df["failure_type"] == "parse_failure").sum())
            other = int((fw_df["failure_type"] == "other_failure").sum())
            rate_rows.append({
                "Framework": fw,
                "Success": successes,
                "Build Fail": builds,
                "Server Fail": startups,
                "Timeout": timeouts,
                "Integrity": integrity,
                "Parse": parse,
                "Other": other,
                "Total": total,
                "Success %": f"{100*successes/total:.0f}%" if total else "—",
            })
        st.dataframe(pd.DataFrame(rate_rows), use_container_width=True, hide_index=True)

        # Per-framework build errors
        build_errors = df[(df["failure_type"] == "build_failure") & df["error"].notna() & (df["error"] != "")]
        if not build_errors.empty:
            st.markdown("### Build Errors by Framework")
            for fw in sorted(build_errors["framework"].unique()):
                fw_errors = build_errors[build_errors["framework"] == fw]
                with st.expander(f"**{fw}** — {len(fw_errors)} build failure(s)"):
                    error_counts = fw_errors["error"].value_counts()
                    for err, count in error_counts.items():
                        st.code(f"[{count}x] {err[:200]}", language="text")

        # Per-framework diagnosis from analysis.json
        st.markdown("### Diagnosis (from post-run analysis)")
        analysis_found = False
        for fw_dir in sorted(RESULTS_DIR.iterdir()):
            if not fw_dir.is_dir():
                continue
            analysis_file = fw_dir / "analysis.json"
            if analysis_file.exists():
                try:
                    analysis = json.loads(analysis_file.read_text())
                    diagnosis = analysis.get("diagnosis", "")
                    if diagnosis:
                        analysis_found = True
                        p2p_status = ""
                        if analysis.get("p2p_attempted"):
                            p2p_status = " | P2P attempted"
                            if analysis.get("p2p_compiled"):
                                p2p_status = " | **P2P compiled!**"
                            else:
                                p2p_status = " | P2P attempted but failed"
                        errors = analysis.get("build_errors", [])
                        error_str = f" | Errors: {', '.join(errors[:2])}" if errors else ""
                        st.markdown(f"**{fw_dir.name}**{p2p_status}: {diagnosis}{error_str}")
                except (json.JSONDecodeError, OSError):
                    pass
        if not analysis_found:
            st.caption("No analysis.json files yet — run the experiment to generate.")


# ============================================================
# TAB 6: Discovery vs Implementation Matrix
# ============================================================
with tab_arch:
    st.subheader("Discovery vs Implementation")

    # Show current pipeline config vs wild-type
    repo_root = RESULTS_DIR.parents[2]
    _pipeline_src = (repo_root / "components" / "dispatcher" / "src" / "pipeline.rs").read_text() if (repo_root / "components" / "dispatcher" / "src" / "pipeline.rs").exists() else ""
    _lib_src = (repo_root / "components" / "dispatcher" / "src" / "lib.rs").read_text() if (repo_root / "components" / "dispatcher" / "src" / "lib.rs").exists() else ""
    _wt_pipeline = (INITIAL_PROGRAMS / "pipeline.rs").read_text() if (INITIAL_PROGRAMS / "pipeline.rs").exists() else ""

    def _extract_val(pattern, text, default="?"):
        m = re.search(pattern, text)
        return m.group(1) if m else default

    _cur_ring = _extract_val(r"PIPELINE_RING_SIZE.*?=\s*(\d+)", _pipeline_src)
    _cur_queues = _extract_val(r"MAX_QUEUES_PER_DRIVE.*?=\s*(\d+)", _lib_src)
    _cur_qd = _extract_val(r"queue_depth.*?=\s*(\d+)", _lib_src)
    _cur_streams = _extract_val(r"GpuStream;\s*(\d+)", _pipeline_src)
    _cur_sync = "No" if "stream_idx % 16" not in _pipeline_src else "Yes (every 16 chunks)"

    _wt_ring = _extract_val(r"PIPELINE_RING_SIZE.*?=\s*(\d+)", _wt_pipeline)
    _wt_streams = _extract_val(r"GpuStream;\s*(\d+)", _wt_pipeline)
    _wt_sync = "Yes (every 16 chunks)" if "stream_idx % 16" in _wt_pipeline else "No"

    st.markdown("#### Pipeline Configuration: Baseline vs Best Result")
    cfg_col1, cfg_col2 = st.columns(2)
    with cfg_col1:
        st.markdown("**Wild-type (baseline)**")
        st.code(
            f"Ring size:        {_wt_ring} DMA buffers\n"
            f"NVMe queues/drive: 1\n"
            f"Queue depth:       16 / num_queues = QD16\n"
            f"CUDA streams:      {_wt_streams}\n"
            f"Mid-pipeline sync: {_wt_sync}\n"
            f"Chunk size:        128 KiB",
            language=None,
        )
    with cfg_col2:
        # Show best result — prefer P2P implementation if one exists
        if not df.empty:
            # Find framework that implemented P2P successfully (if any)
            p2p_fw = None
            for _fw_name in df["framework"].unique():
                _af = RESULTS_DIR / _fw_name / "analysis.json"
                if _af.exists():
                    try:
                        _ad = json.loads(_af.read_text())
                        if _ad.get("p2p_compiled") and _ad.get("beat_baseline"):
                            p2p_fw = _fw_name
                            break
                    except (json.JSONDecodeError, OSError):
                        pass

            if p2p_fw:
                # Show P2P implementor as the featured result
                _p2p_df = df[df["framework"] == p2p_fw]
                best_score = _p2p_df["score"].max()
                best_tp = _p2p_df["throughput_gbps"].max() if _p2p_df["throughput_gbps"].notna().any() else None
                tp_str = f", {best_tp:.2f} GB/s" if best_tp else ""
                st.markdown(f"**Best P2P: {p2p_fw} (score {best_score:.4f}{tp_str})**")
                config_text = (
                    "Architecture:     P2P (NVMe → GPU BAR1 → D2D)\n"
                    "P2P ring:         64 GDRCopy BAR1-mapped GPU slots\n"
                    "NVMe DMA target:  GPU VRAM via spdk_mem_register\n"
                    "Staging copy:     cudaMemcpyAsync D2D (<1us)\n"
                    "CUDA streams:     2 (alternating)\n"
                    "Mid-pipeline sync: Lazy (only on ring recycle)\n"
                    "Chunk size:       128 KiB\n"
                    "Host DRAM bounce: Eliminated"
                )
            else:
                # No P2P — show highest scorer
                best_fw = df.loc[df["score"].idxmax(), "framework"]
                best_score = df["score"].max()
                best_tp = df.loc[df["score"].idxmax(), "throughput_gbps"]
                tp_str = f", {best_tp:.2f} GB/s" if pd.notna(best_tp) else ""
                st.markdown(f"**Best: {best_fw} (score {best_score:.4f}{tp_str})**")
                config_text = (
                    f"Ring size:        {_wt_ring} DMA buffers\n"
                    f"NVMe queues/drive: {_cur_queues}\n"
                    f"Queue depth:       {_cur_qd}\n"
                    f"CUDA streams:      {_cur_streams}\n"
                    f"Mid-pipeline sync: {_cur_sync}\n"
                    f"Chunk size:        128 KiB"
                )
            st.code(config_text, language=None)
        else:
            st.info("No results yet")

    st.markdown("---")
    st.caption("Did the framework identify P2P as an option? Did it attempt it? Did it compile? Did it improve?")

    p2p_markers = [
        "gpu_direct", "gpudirect", "peer_mem", "nvidia_peermem",
        "p2p_", "nv_p2p", "gdr", "cuFile", "bar_memory", "gpu_dma",
        "spdk_mem_register", "GpuDirectBuffer",
        "create_spdk_dma_buffer_from_gpu_bar", "create_spdk_dma_buffer_from_gpu",
    ]

    matrix_rows = []
    wild_type = load_wild_type()

    for fw in sorted(
        df["framework"].unique(),
        key=lambda f: df[df["framework"] == f]["throughput_gbps"].max() if df[df["framework"] == f]["throughput_gbps"].notna().any() else 0,
        reverse=True,
    ):
        fw_df = df[df["framework"] == fw]
        fw_best = fw_df["score"].max()
        has_successes = (fw_df["failure_type"] == "success").any()
        beat_baseline = fw_best > WILD_TYPE_SCORE

        # Read P2P status from analysis.json (authoritative source)
        fw_dir = RESULTS_DIR / fw
        _fw_analysis_file = fw_dir / "analysis.json"
        _fw_analysis = {}
        if _fw_analysis_file.exists():
            try:
                _fw_analysis = json.loads(_fw_analysis_file.read_text())
            except (json.JSONDecodeError, OSError):
                pass

        discovered_p2p = _fw_analysis.get("p2p_attempted", False) or _fw_analysis.get("p2p_compiled", False)
        attempted_p2p = _fw_analysis.get("p2p_attempted", False)

        # Classify best compiled candidate
        best_type = "—"
        if has_successes and wild_type:
            # Find best candidate source
            candidate_files = {}
            output_dir = fw_dir / "output"
            candidates_dir = fw_dir / "candidates"
            best_dir = fw_dir / "best"

            best_eval = int(fw_df.loc[fw_df["score"].idxmax(), "eval_num"])
            # Try candidates/gen_N/main.rs (concatenated format)
            gen_main = candidates_dir / f"gen_{best_eval}" / "main.rs"
            if gen_main.exists():
                candidate_files = split_concatenated(gen_main.read_text())
            # Try candidates/gen_N/*.rs (individual files — coding_agent format)
            if not candidate_files:
                gen_dir = candidates_dir / f"gen_{best_eval}"
                if gen_dir.exists():
                    for f in gen_dir.iterdir():
                        if f.suffix == ".rs":
                            candidate_files[f.name] = f.read_text()
            # Fallback: use highest available gen if exact best_eval doesn't exist
            if not candidate_files and candidates_dir.exists():
                gen_dirs = sorted(
                    [d for d in candidates_dir.iterdir() if d.is_dir() and d.name.startswith("gen_")],
                    key=lambda d: int(d.name.split("_")[1]) if d.name.split("_")[1].isdigit() else 0,
                    reverse=True,
                )
                for gd in gen_dirs:
                    main_rs = gd / "main.rs"
                    if main_rs.exists():
                        candidate_files = split_concatenated(main_rs.read_text())
                        break
                    rs_files = list(gd.glob("*.rs"))
                    if rs_files:
                        for f in rs_files:
                            candidate_files[f.name] = f.read_text()
                        break
            if not candidate_files and output_dir.exists():
                gen_main = output_dir / f"gen_{best_eval}" / "main.rs"
                if gen_main.exists():
                    candidate_files = split_concatenated(gen_main.read_text())
            if not candidate_files and best_dir.exists():
                for f in best_dir.iterdir():
                    if f.suffix == ".rs":
                        candidate_files[f.name] = f.read_text()

            # For agentic frameworks without saved candidates, classify from analysis.json
            # (Working tree may be modified by a currently-running experiment)
            if not candidate_files and fw in ("nous", "autoscientists"):
                if _fw_analysis.get("p2p_compiled") and _fw_analysis.get("beat_baseline"):
                    best_type = "P2P (compiled, faster)"
                elif _fw_analysis.get("p2p_compiled"):
                    best_type = "P2P (compiled, slower)"
                elif _fw_analysis.get("beat_baseline"):
                    best_type = "Pipeline restructure"
                else:
                    best_type = "No improvement"
            elif candidate_files:
                result = classify_architecture(candidate_files, wild_type)
                cat = result["category"]
                if cat == "path_change":
                    best_type = "P2P"
                elif cat == "pipeline_restructure":
                    best_type = "Pipeline restructure"
                elif cat == "hybrid":
                    best_type = "Hybrid (pipeline + knobs)"
                elif cat == "knob_tuning":
                    best_type = "Knob tuning"
                elif result["lines_added"] == 0 and result["lines_removed"] == 0:
                    best_type = "Unchanged seed"
                else:
                    best_type = "Structural"
            else:
                best_type = "No improvement" if has_successes else "—"

        # Override best_type from analysis.json only if P2P actually compiled and ran
        if _fw_analysis.get("p2p_compiled"):
            best_type = "P2P (compiled, " + ("faster" if _fw_analysis.get("beat_baseline") else "slower") + ")"

        # Use diagnosis from analysis.json as the strategy description
        best_strategy = _fw_analysis.get("diagnosis", "—")

        # P2P compiled status from analysis.json (authoritative)
        p2p_compiled = _fw_analysis.get("p2p_compiled", False)

        matrix_rows.append({
            "Framework": fw,
            "Discovered P2P": "Yes" if discovered_p2p else "No",
            "Attempted P2P": "Yes" if attempted_p2p else "No",
            "P2P Compiled": "Yes" if p2p_compiled else "No",
            "Compiled (any)": "Yes" if has_successes else "No",
            "Best Type": best_type,
            "Beat Baseline": "Yes" if beat_baseline else "No",
            "Best Throughput": f"{fw_df['throughput_gbps'].max():.2f} GB/s" if fw_df["throughput_gbps"].notna().any() else "—",
            "What Worked": best_strategy,
        })

    if matrix_rows:
        matrix_df = pd.DataFrame(matrix_rows)

        # Replace Yes/No with colored emoji indicators for readability
        bool_cols = ["Discovered P2P", "Attempted P2P", "P2P Compiled", "Compiled (any)", "Beat Baseline"]
        for col in bool_cols:
            matrix_df[col] = matrix_df[col].map(
                lambda v: "✅" if v == "Yes" else "❌"
            )

        # Show compact matrix (without long text)
        display_cols = ["Framework", "Discovered P2P", "Attempted P2P", "P2P Compiled",
                        "Compiled (any)", "Best Type", "Beat Baseline", "Best Throughput"]
        st.dataframe(matrix_df[display_cols], use_container_width=True, hide_index=True)

        # Show key differentiator — what actually caused the throughput difference
        st.markdown("#### Key Differentiator (why scores differ)")
        st.markdown(
            "The single biggest factor is **total NVMe pipeline depth** "
            "(commands in-flight). Secondary: removing periodic GPU sync stalls."
        )

        # Build key differentiator per framework from candidates
        for _, row in matrix_df.iterrows():
            fw = row["Framework"]
            fw_df_arch = df[df["framework"] == fw]
            if fw_df_arch.empty or fw_df_arch["score"].max() <= WILD_TYPE_SCORE:
                continue

            # Try to extract queue config from best candidate
            best_eval = int(fw_df_arch.loc[fw_df_arch["score"].idxmax(), "eval_num"])
            fw_dir = RESULTS_DIR / fw
            candidates_dir = fw_dir / "candidates"
            gen_dir = candidates_dir / f"gen_{best_eval}"

            key_config = ""
            # Check lib.rs for queue config
            lib_file = None
            if gen_dir.exists():
                lib_file = gen_dir / "lib.rs"
            if lib_file and lib_file.exists():
                import re as _re
                lib_text = lib_file.read_text()
                queues_m = _re.search(r"MAX_QUEUES_PER_DRIVE.*?(\d+)", lib_text)
                qd_m = _re.search(r"queue_depth.*?=\s*(\d+)", lib_text)
                if queues_m and qd_m:
                    q = int(queues_m.group(1))
                    d = int(qd_m.group(1))
                    key_config = f"{q}Q × QD{d} = {q*d} in-flight"

            # Check pipeline.rs for sync removal
            pipeline_file = None
            if gen_dir.exists():
                pipeline_file = gen_dir / "pipeline.rs"
            sync_removed = ""
            if pipeline_file and pipeline_file.exists():
                if "stream_idx % 16" not in pipeline_file.read_text():
                    sync_removed = " + no mid-sync"

            # Fallback: known configs for frameworks without saved candidates
            if not key_config:
                known_configs = {
                    "nous": "4Q × QD32 = 128 in-flight + no mid-sync + 4 CUDA streams",
                    "autoscientists": "2Q × QD32 = 64 in-flight + no mid-sync + pre-alloc MT slots",
                    "coding_agent_sdk": "2Q × QD64 = 128 in-flight + batch pipeline + 3 queues",
                }
                key_config = known_configs.get(fw, "")

            if key_config:
                st.markdown(f"**{fw}** ({row['Best Throughput']}) — `{key_config}{sync_removed}`")
            else:
                strategy = row.get("What Worked", "—")
                if strategy and strategy != "—":
                    st.markdown(f"**{fw}** ({row['Best Throughput']})")

        st.markdown("")
        st.markdown("#### Detailed Strategy per Framework")
        for _, row in matrix_df.iterrows():
            fw_name = row["Framework"]
            strategy = row.get("What Worked", "—")
            best_type = row["Best Type"]
            throughput = row.get("Best Throughput", "—")

            # Try to load diagnosis from analysis.json
            analysis_diagnosis = ""
            analysis_file = RESULTS_DIR / fw_name / "analysis.json"
            if analysis_file.exists():
                try:
                    analysis_diagnosis = json.loads(analysis_file.read_text()).get("diagnosis", "")
                except (json.JSONDecodeError, OSError):
                    pass

            if analysis_diagnosis:
                st.markdown(f"**{fw_name}** ({throughput}) — {analysis_diagnosis}")
            elif strategy and strategy != "—":
                st.markdown(f"**{fw_name}** ({throughput}) — _{best_type}_ — {strategy}")
            elif best_type == "No improvement":
                st.markdown(f"**{fw_name}** ({throughput}) — Compiled but no improvement over baseline.")
            elif best_type not in ("—", "?"):
                st.markdown(f"**{fw_name}** ({throughput}) — _{best_type}_")

        # Data-driven key finding based on P2P compilation status
        st.markdown("---")
        p2p_compiled_any = any(row.get("P2P Compiled") == "✅" for _, row in matrix_df.iterrows())
        if p2p_compiled_any:
            st.markdown("**Key finding:** At least one framework successfully compiled and ran P2P code.")
        else:
            n_attempted = sum(1 for _, row in matrix_df.iterrows() if row.get("Attempted P2P") == "✅")
            n_discovered = sum(1 for _, row in matrix_df.iterrows() if row.get("Discovered P2P") == "✅")
            st.markdown(
                f"**Key finding:** {n_discovered} frameworks discovered P2P, "
                f"{n_attempted} attempted it, **none compiled it successfully.** "
                f"Best results came from optimizing within the host-bounce architecture."
            )

# --- Sidebar ---
with st.sidebar:
    st.markdown("### Experiment Info")
    st.markdown(f"**Results dir**: `{RESULTS_DIR}`")
    st.markdown(f"**Wild-type**: {WILD_TYPE_SCORE:.4f}")
    st.markdown(f"**Scoring**: 60% throughput + 40% latency")
    st.markdown("---")
    st.markdown("### Hardware Ceilings (measured)")
    st.markdown(f"**Config**: {DRIVES_IN_CONFIG} drive(s), {CLIENTS_IN_CONFIG} client(s)")
    st.markdown(f"**Single drive**: {SINGLE_DRIVE_GBPS} GB/s")
    st.markdown(f"**NVMe aggregate**: {NVME_AGGREGATE_GBPS:.1f} GB/s")
    st.markdown(f"**H2D (4 MiB)**: {H2D_4MIB_GBPS} GB/s")
    st.markdown(f"**Actual ceiling**: {ACTUAL_CEILING_GBPS:.1f} GB/s")
    st.markdown(f"**Latency target**: {LATENCY_TARGET_MS} ms p99")
    st.markdown("---")
    if st.button("Refresh data"):
        st.cache_data.clear()
        st.rerun()
    st.markdown("---")
    st.caption("Auto-refreshes every 10s (cache TTL)")

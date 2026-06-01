#!/usr/bin/env python3
"""Evolve P2P Experiment Dashboard.

Streamlit app that reads local result files and shows:
1. Overview — summary table + top-line cards
2. Trajectories — score vs evaluation number per framework
3. Pareto — throughput vs latency scatter with Pareto frontier
4. Candidate Details — click into individual evaluations

Run:
    streamlit run dashboard.py
"""
from __future__ import annotations

import json
from pathlib import Path

import pandas as pd
import plotly.express as px
import plotly.graph_objects as go
import streamlit as st

RESULTS_DIR = Path(__file__).resolve().parent / "results"
WILD_TYPE_SCORE = 0.2026
THROUGHPUT_CEILING = 12.0
LATENCY_TARGET_MS = 0.4

FRAMEWORK_COLORS = {
    "gepa_native": "#1f77b4",
    "adaevolve": "#ff7f0e",
    "evox": "#2ca02c",
    "openevolve": "#d62728",
    "nous": "#9467bd",
    "autoscientists": "#8c564b",
    "random": "#7f7f7f",
}


@st.cache_data(ttl=10)
def load_all_scores() -> pd.DataFrame:
    """Load scores.jsonl from all framework result dirs."""
    rows = []
    if not RESULTS_DIR.exists():
        return pd.DataFrame()

    for fw_dir in sorted(RESULTS_DIR.iterdir()):
        if not fw_dir.is_dir() or fw_dir.name == "summary":
            continue
        framework = fw_dir.name

        scores_file = fw_dir / "scores.jsonl"
        if scores_file.exists():
            for i, line in enumerate(scores_file.read_text().splitlines()):
                try:
                    entry = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                row = {
                    "framework": framework,
                    "eval_num": entry.get("iteration", i),
                    "score": entry.get("combined_score", 0.0),
                    "throughput_gbps": entry.get("throughput_gbps"),
                    "p99_latency_ms": entry.get("p99_latency_ms"),
                    "p50_latency_ms": entry.get("p50_latency_ms"),
                    "mean_latency_ms": entry.get("mean_latency_ms"),
                    "cpu_util_fraction": entry.get("cpu_util_fraction"),
                    "build_succeeded": entry.get("build_succeeded", True),
                    "data_integrity": entry.get("data_integrity", True),
                }
                rows.append(row)

        # Also check SkyDiscover output dir
        output_dir = fw_dir / "output"
        if output_dir.exists():
            for jsonl_file in sorted(output_dir.rglob("*.jsonl")):
                for i, line in enumerate(jsonl_file.read_text().splitlines()):
                    try:
                        entry = json.loads(line)
                    except (json.JSONDecodeError, ValueError):
                        continue
                    score = entry.get("combined_score") or entry.get("score", 0.0)
                    row = {
                        "framework": framework,
                        "eval_num": entry.get("iteration", i),
                        "score": score,
                        "throughput_gbps": entry.get("throughput_gbps"),
                        "p99_latency_ms": entry.get("p99_latency_ms"),
                        "p50_latency_ms": entry.get("p50_latency_ms"),
                        "mean_latency_ms": entry.get("mean_latency_ms"),
                        "cpu_util_fraction": entry.get("cpu_util_fraction"),
                        "build_succeeded": entry.get("build_succeeded", True),
                        "data_integrity": entry.get("data_integrity", True),
                    }
                    rows.append(row)

    if not rows:
        return pd.DataFrame()

    df = pd.DataFrame(rows)
    df["score"] = pd.to_numeric(df["score"], errors="coerce").fillna(0.0)
    return df


def compute_best_so_far(df: pd.DataFrame) -> pd.DataFrame:
    """Add cumulative best score per framework."""
    if df.empty:
        return df
    df = df.sort_values(["framework", "eval_num"]).copy()
    df["best_so_far"] = df.groupby("framework")["score"].cummax()
    return df


def load_summaries() -> list[dict]:
    """Load per-framework summary.json files."""
    summaries = []
    if not RESULTS_DIR.exists():
        return summaries
    for fw_dir in sorted(RESULTS_DIR.iterdir()):
        if not fw_dir.is_dir() or fw_dir.name == "summary":
            continue
        summary_file = fw_dir / "summary.json"
        if summary_file.exists():
            try:
                summaries.append(json.loads(summary_file.read_text()))
            except (json.JSONDecodeError, ValueError):
                continue
    return summaries


# --- Page config ---
st.set_page_config(
    page_title="Evolve P2P Dashboard",
    page_icon="🧬",
    layout="wide",
)
st.title("Evolve P2P — Directed Evolution Dashboard")

# Load data
df = load_all_scores()

if df.empty:
    st.warning(
        "No results found. Run the experiment first:\n\n"
        "```\ncd evolution/evolve_p2p\n"
        "python run_experiment.py --frameworks gepa_native,random --iterations 10\n```"
    )
    st.stop()

df = compute_best_so_far(df)

# --- Tabs ---
tab_overview, tab_trajectory, tab_pareto, tab_details = st.tabs(
    ["Overview", "Trajectories", "Pareto", "Candidate Details"]
)

# ============================================================
# TAB 1: Overview
# ============================================================
with tab_overview:
    frameworks = df["framework"].unique().tolist()

    # Top-line cards
    col1, col2, col3, col4 = st.columns(4)
    best_row = df.loc[df["score"].idxmax()]
    total_evals = len(df)
    build_fails = (~df["build_succeeded"]).sum() if "build_succeeded" in df.columns else 0
    integrity_fails = (~df["data_integrity"]).sum() if "data_integrity" in df.columns else 0

    col1.metric("Wild-type score", f"{WILD_TYPE_SCORE:.4f}")
    col2.metric("Best score", f"{best_row['score']:.4f}",
                delta=f"+{best_row['score'] - WILD_TYPE_SCORE:.4f}")
    col3.metric("Best framework", best_row["framework"])
    col4.metric("Total evaluations", total_evals)

    col5, col6, col7 = st.columns(3)
    col5.metric("Build failures", int(build_fails))
    col6.metric("Integrity failures", int(integrity_fails))
    col7.metric("Frameworks tested", len(frameworks))

    st.markdown("---")

    # Summary table
    summary_rows = []
    for fw in sorted(frameworks):
        fw_df = df[df["framework"] == fw]
        best = fw_df["score"].max()
        best_iter = fw_df.loc[fw_df["score"].idxmax(), "eval_num"] if not fw_df.empty else 0
        throughput = fw_df["throughput_gbps"].max() if fw_df["throughput_gbps"].notna().any() else None
        p99 = fw_df.loc[fw_df["score"].idxmax(), "p99_latency_ms"] if fw_df["p99_latency_ms"].notna().any() else None
        n_build_fail = (~fw_df["build_succeeded"]).sum()
        n_integrity_fail = (~fw_df["data_integrity"]).sum()

        summary_rows.append({
            "Framework": fw,
            "Best Score": f"{best:.4f}",
            "Δ over baseline": f"+{best - WILD_TYPE_SCORE:.4f}" if best > WILD_TYPE_SCORE else f"{best - WILD_TYPE_SCORE:.4f}",
            "Throughput (GB/s)": f"{throughput:.2f}" if throughput else "—",
            "p99 Latency (ms)": f"{p99:.3f}" if p99 else "—",
            "Build Fails": int(n_build_fail),
            "Integrity Fails": int(n_integrity_fail),
            "Evals": len(fw_df),
            "Best Iteration": int(best_iter),
        })

    st.dataframe(pd.DataFrame(summary_rows), use_container_width=True, hide_index=True)

# ============================================================
# TAB 2: Trajectories
# ============================================================
with tab_trajectory:
    st.subheader("Score Trajectory (Best-so-far)")

    fig = go.Figure()

    for fw in sorted(df["framework"].unique()):
        fw_df = df[df["framework"] == fw].sort_values("eval_num")
        color = FRAMEWORK_COLORS.get(fw, "#333333")

        # Raw scores as faint dots
        fig.add_trace(go.Scatter(
            x=fw_df["eval_num"],
            y=fw_df["score"],
            mode="markers",
            marker=dict(size=5, color=color, opacity=0.25),
            name=f"{fw} (raw)",
            legendgroup=fw,
            showlegend=False,
        ))

        # Best-so-far as solid line
        fig.add_trace(go.Scatter(
            x=fw_df["eval_num"],
            y=fw_df["best_so_far"],
            mode="lines",
            line=dict(width=2.5, color=color),
            name=fw,
            legendgroup=fw,
        ))

    # Wild-type baseline
    fig.add_hline(y=WILD_TYPE_SCORE, line_dash="dash", line_color="red",
                  annotation_text="Wild-type baseline")

    fig.update_layout(
        xaxis_title="Evaluation Number",
        yaxis_title="Score",
        height=500,
        legend=dict(orientation="h", yanchor="bottom", y=1.02),
    )
    st.plotly_chart(fig, use_container_width=True)

    # Show per-framework detail
    with st.expander("Per-framework raw data"):
        for fw in sorted(df["framework"].unique()):
            fw_df = df[df["framework"] == fw]
            st.write(f"**{fw}**: {len(fw_df)} evals, "
                     f"best={fw_df['score'].max():.4f}, "
                     f"mean={fw_df['score'].mean():.4f}")

# ============================================================
# TAB 3: Pareto (Throughput vs Latency)
# ============================================================
with tab_pareto:
    st.subheader("Throughput vs p99 Latency (Pareto)")

    pareto_df = df[df["throughput_gbps"].notna() & df["p99_latency_ms"].notna()].copy()

    if pareto_df.empty:
        st.info("No throughput/latency data available yet. "
                "The scores.jsonl needs full metrics (run with enhanced evaluator).")
    else:
        fig = px.scatter(
            pareto_df,
            x="p99_latency_ms",
            y="throughput_gbps",
            color="framework",
            color_discrete_map=FRAMEWORK_COLORS,
            hover_data=["eval_num", "score"],
            labels={
                "p99_latency_ms": "p99 Latency (ms) — lower is better →",
                "throughput_gbps": "Throughput (GB/s) — higher is better ↑",
            },
        )

        # Compute and plot Pareto frontier
        sorted_pareto = pareto_df.sort_values("p99_latency_ms")
        frontier = []
        max_throughput = -1
        for _, row in sorted_pareto.iterrows():
            if row["throughput_gbps"] > max_throughput:
                max_throughput = row["throughput_gbps"]
                frontier.append(row)

        if frontier:
            frontier_df = pd.DataFrame(frontier)
            fig.add_trace(go.Scatter(
                x=frontier_df["p99_latency_ms"],
                y=frontier_df["throughput_gbps"],
                mode="lines",
                line=dict(width=2, color="black", dash="dot"),
                name="Pareto frontier",
                showlegend=True,
            ))

        # Mark wild-type (approximate from baselines: 1 drive, 1 client)
        fig.add_trace(go.Scatter(
            x=[1.753], y=[2.39],
            mode="markers",
            marker=dict(size=14, symbol="diamond", color="red", line=dict(width=2, color="black")),
            name="Wild-type (1d/1c)",
        ))

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
    fw_df = df[df["framework"] == sel_framework].sort_values("eval_num")

    if fw_df.empty:
        st.info("No data for this framework.")
    else:
        sel_eval = col_iter.selectbox(
            "Evaluation",
            fw_df["eval_num"].tolist(),
            index=int(fw_df["score"].idxmax() - fw_df.index[0]) if len(fw_df) > 0 else 0,
            format_func=lambda x: f"Eval {x} (score: {fw_df[fw_df['eval_num'] == x]['score'].values[0]:.4f})",
        )

        candidate = fw_df[fw_df["eval_num"] == sel_eval].iloc[0]

        # Metrics display
        st.markdown("### Metrics")
        mc1, mc2, mc3, mc4 = st.columns(4)
        mc1.metric("Score", f"{candidate['score']:.4f}")
        mc2.metric("Throughput", f"{candidate['throughput_gbps']:.2f} GB/s" if pd.notna(candidate.get("throughput_gbps")) else "—")
        mc3.metric("p99 Latency", f"{candidate['p99_latency_ms']:.3f} ms" if pd.notna(candidate.get("p99_latency_ms")) else "—")
        mc4.metric("CPU Util", f"{candidate['cpu_util_fraction']:.1%}" if pd.notna(candidate.get("cpu_util_fraction")) else "—")

        mc5, mc6, mc7 = st.columns(3)
        mc5.metric("p50 Latency", f"{candidate['p50_latency_ms']:.3f} ms" if pd.notna(candidate.get("p50_latency_ms")) else "—")
        mc6.metric("Build", "PASS" if candidate.get("build_succeeded") else "FAIL")
        mc7.metric("Integrity", "PASS" if candidate.get("data_integrity") else "FAIL")

        # Show candidate source if available
        st.markdown("### Candidate Source")
        candidates_dir = RESULTS_DIR / sel_framework / "candidates"
        if candidates_dir.exists():
            eval_idx = sel_eval + 1  # 1-indexed file naming
            found_files = []
            for suffix in ["pipeline.rs", "lib.rs", "dma.rs"]:
                candidate_file = candidates_dir / f"{eval_idx:03d}_{suffix}"
                if candidate_file.exists():
                    found_files.append((suffix, candidate_file))

            if found_files:
                for name, path in found_files:
                    with st.expander(f"{name}"):
                        st.code(path.read_text()[:5000], language="rust")
            else:
                st.info(f"No candidate files found for eval {sel_eval} "
                        f"(looked in {candidates_dir})")
        else:
            # Check for best/ directory
            best_dir = RESULTS_DIR / sel_framework / "best"
            if best_dir.exists():
                st.info("Showing best candidate (per-eval files not stored):")
                for f in sorted(best_dir.iterdir()):
                    if f.suffix == ".rs":
                        with st.expander(f.name):
                            st.code(f.read_text()[:5000], language="rust")
            else:
                st.info("No candidate source files available.")

# --- Sidebar ---
with st.sidebar:
    st.markdown("### Experiment Info")
    st.markdown(f"**Results dir**: `{RESULTS_DIR}`")
    st.markdown(f"**Wild-type**: {WILD_TYPE_SCORE:.4f}")
    st.markdown(f"**Scoring**: 60% throughput + 40% latency")
    st.markdown(f"**Ceiling**: {THROUGHPUT_CEILING} GB/s throughput")
    st.markdown(f"**Target**: {LATENCY_TARGET_MS} ms p99 latency")
    st.markdown("---")
    if st.button("Refresh data"):
        st.cache_data.clear()
        st.rerun()
    st.markdown("---")
    st.caption("Auto-refreshes every 10s (cache TTL)")

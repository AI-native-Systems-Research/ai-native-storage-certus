#!/usr/bin/env python3
"""Generate a standalone HTML report from evolve-p2p experiment results.

Usage:
    python3 generate_report.py
    # Produces report.html in the same directory
"""
import json
from datetime import datetime
from pathlib import Path

import plotly.graph_objects as go

EXPERIMENT_DIR = Path(__file__).resolve().parent
RESULTS_NO_HINT = EXPERIMENT_DIR / "results"
RESULTS_HINT = EXPERIMENT_DIR / "results_hint"
WILD_TYPE_SCORE = 0.2026
ACTUAL_CEILING_GBPS = 5.92

FRAMEWORK_COLORS = {
    "gepa_native": "#1f77b4",
    "adaevolve": "#ff7f0e",
    "evox": "#2ca02c",
    "openevolve": "#d62728",
    "shinkaevolve": "#9467bd",
    "ksearch": "#8c564b",
    "nous": "#e377c2",
    "autoscientists": "#bcbd22",
    "coding_agent": "#17becf",
    "coding_agent_sdk": "#7f7f7f",
}


def load_scores(results_dir: Path) -> list[dict]:
    rows = []
    if not results_dir.exists():
        return rows
    for fw_dir in sorted(results_dir.iterdir()):
        if not fw_dir.is_dir():
            continue
        scores_file = fw_dir / "scores.jsonl"
        if scores_file.exists():
            for line in scores_file.read_text().splitlines():
                try:
                    entry = json.loads(line)
                    entry["framework"] = fw_dir.name
                    rows.append(entry)
                except (json.JSONDecodeError, ValueError):
                    continue
    return rows


def load_analyses(results_dir: Path) -> dict[str, dict]:
    analyses = {}
    if not results_dir.exists():
        return analyses
    for fw_dir in sorted(results_dir.iterdir()):
        if not fw_dir.is_dir():
            continue
        af = fw_dir / "analysis.json"
        if af.exists():
            try:
                analyses[fw_dir.name] = json.loads(af.read_text())
            except (json.JSONDecodeError, OSError):
                pass
    return analyses


def load_summaries(results_dir: Path) -> dict[str, dict]:
    summaries = {}
    if not results_dir.exists():
        return summaries
    for fw_dir in sorted(results_dir.iterdir()):
        if not fw_dir.is_dir():
            continue
        sf = fw_dir / "summary.json"
        if sf.exists():
            try:
                summaries[fw_dir.name] = json.loads(sf.read_text())
            except (json.JSONDecodeError, OSError):
                pass
    return summaries


def make_overview_table(scores: list[dict], analyses: dict, summaries: dict) -> str:
    frameworks = {}
    for row in scores:
        fw = row["framework"]
        if fw not in frameworks:
            frameworks[fw] = []
        frameworks[fw].append(row)

    sorted_fws = sorted(frameworks.keys(),
                        key=lambda f: max(r.get("combined_score", 0) for r in frameworks[f]),
                        reverse=True)

    html = '<table class="data-table">\n<thead><tr>'
    html += "<th>Framework</th><th>Best Score</th><th>Throughput</th><th>P2P</th><th>Evals</th><th>Wall Time</th><th>Diagnosis</th>"
    html += "</tr></thead>\n<tbody>\n"

    for fw in sorted_fws:
        rows = frameworks[fw]
        best_score = max(r.get("combined_score", 0) for r in rows)
        throughputs = [r.get("throughput_gbps") for r in rows if r.get("throughput_gbps")]
        best_tp = f"{max(throughputs):.2f} GB/s" if throughputs else "—"

        analysis = analyses.get(fw, {})
        if analysis.get("p2p_compiled") and analysis.get("beat_baseline"):
            p2p = "P2P (faster)"
        elif analysis.get("p2p_compiled"):
            p2p = "P2P (slower)"
        elif analysis.get("p2p_attempted"):
            p2p = "Attempted"
        else:
            p2p = "No"

        summary = summaries.get(fw, {})
        wall = summary.get("wall_time_seconds")
        wall_str = f"{wall/60:.0f}m" if wall else "—"
        diagnosis = analysis.get("diagnosis", "—")

        html += f"<tr><td><strong>{fw}</strong></td><td>{best_score:.4f}</td><td>{best_tp}</td>"
        html += f"<td>{p2p}</td><td>{len(rows)}</td><td>{wall_str}</td>"
        html += f'<td class="diagnosis">{diagnosis}</td></tr>\n'

    html += "</tbody></table>"
    return html


def make_trajectory_chart(scores: list[dict], title: str) -> str:
    fig = go.Figure()
    frameworks = {}
    for row in scores:
        fw = row["framework"]
        if fw not in frameworks:
            frameworks[fw] = []
        frameworks[fw].append(row)

    for fw in sorted(frameworks.keys()):
        rows = sorted(frameworks[fw], key=lambda r: r.get("iteration", r.get("eval_num", 0)))
        # Best per iteration for multi-arm
        iters = {}
        for r in rows:
            it = r.get("iteration", r.get("eval_num", 0))
            score = r.get("combined_score", 0)
            if it not in iters or score > iters[it]:
                iters[it] = score
        x = sorted(iters.keys())
        y = [iters[i] for i in x]

        color = FRAMEWORK_COLORS.get(fw, "#333333")
        fig.add_trace(go.Scatter(
            x=x, y=y, mode="lines+markers",
            line=dict(width=2, color=color),
            marker=dict(size=5, color=color),
            name=fw,
        ))

    fig.add_hline(y=WILD_TYPE_SCORE, line_dash="dash", line_color="red",
                  annotation_text="Baseline")
    fig.update_layout(
        title=title,
        xaxis_title="Iteration", yaxis_title="Score",
        height=450, width=900,
        legend=dict(orientation="h", yanchor="bottom", y=1.02),
    )
    return fig.to_html(full_html=False, include_plotlyjs=False)


def make_pareto_chart(scores: list[dict], title: str) -> str:
    fig = go.Figure()
    frameworks = {}
    for row in scores:
        tp = row.get("throughput_gbps")
        p99 = row.get("p99_latency_ms")
        if tp and p99 and tp > 0 and p99 > 0:
            fw = row["framework"]
            if fw not in frameworks:
                frameworks[fw] = {"x": [], "y": []}
            frameworks[fw]["x"].append(tp)
            frameworks[fw]["y"].append(p99)

    for fw in sorted(frameworks.keys()):
        color = FRAMEWORK_COLORS.get(fw, "#333333")
        fig.add_trace(go.Scatter(
            x=frameworks[fw]["x"], y=frameworks[fw]["y"],
            mode="markers", marker=dict(size=8, color=color),
            name=fw,
        ))

    fig.update_layout(
        title=title,
        xaxis_title="Throughput (GB/s)", yaxis_title="p99 Latency (ms)",
        height=450, width=900,
        legend=dict(orientation="h", yanchor="bottom", y=1.02),
    )
    return fig.to_html(full_html=False, include_plotlyjs=False)


def make_architecture_matrix(analyses: dict) -> str:
    html = '<table class="data-table">\n<thead><tr>'
    html += "<th>Framework</th><th>Discovered P2P</th><th>Attempted P2P</th><th>P2P Compiled</th><th>Beat Baseline</th><th>Best Type</th>"
    html += "</tr></thead>\n<tbody>\n"

    sorted_fws = sorted(analyses.keys(),
                        key=lambda f: analyses[f].get("best_score", 0),
                        reverse=True)

    for fw in sorted_fws:
        a = analyses[fw]
        discovered = "Yes" if a.get("p2p_attempted") or a.get("p2p_compiled") else "No"
        attempted = "Yes" if a.get("p2p_attempted") else "No"
        compiled = "Yes" if a.get("p2p_compiled") else "No"
        beat = "Yes" if a.get("beat_baseline") else "No"

        if a.get("p2p_compiled") and a.get("beat_baseline"):
            best_type = "P2P (compiled, faster)"
        elif a.get("p2p_compiled"):
            best_type = "P2P (compiled, slower)"
        elif a.get("beat_baseline"):
            best_type = "Pipeline restructure"
        else:
            best_type = "No improvement"

        html += f"<tr><td><strong>{fw}</strong></td><td>{discovered}</td><td>{attempted}</td>"
        html += f"<td>{compiled}</td><td>{beat}</td><td>{best_type}</td></tr>\n"

    html += "</tbody></table>"
    return html


FINDINGS_HINT = """
<h3>Baseline</h3>
<p>2.4 GB/s, score 0.20 (1 queue x QD16, NVMe &rarr; DRAM &rarr; GPU host-bounce path)</p>

<h3>What worked</h3>
<table class="data-table">
<thead><tr><th>Framework</th><th>Score</th><th>Throughput</th><th>Approach</th></tr></thead>
<tbody>
<tr><td><strong>autoscientists</strong></td><td>0.3945</td><td>4.65 GB/s</td><td>P2P failed at runtime &rarr; fell back to host-bounce (QD64, multi-object pipeline, overlapped H2D)</td></tr>
<tr><td><strong>coding_agent</strong></td><td>0.3431</td><td>3.97 GB/s</td><td><strong>P2P implemented successfully</strong> &mdash; GDRCopy BAR1 staging ring + D2D copy</td></tr>
<tr><td><strong>gepa_native</strong></td><td>0.3314</td><td>3.87 GB/s</td><td>Host-bounce only (QD/sync tuning). Never called P2P functions.</td></tr>
</tbody></table>

<h3>P2P implementation: coding_agent</h3>
<pre>NVMe &rarr; GPU BAR1 staging ring (64 slots via cudaMalloc + GDRCopy) &rarr; cudaMemcpyAsync D2D &rarr; final gpu_dst</pre>
<ul>
<li>Pre-allocates 64 GPU memory slots, maps into BAR1 via GDRCopy, registers with SPDK</li>
<li>NVMe controller DMAs directly into GPU BAR1 memory (single PCIe hop)</li>
<li>D2D copy at GPU internal bandwidth (~600 GB/s for 128KiB) &mdash; effectively zero-cost</li>
<li>Host DRAM completely eliminated from data path</li>
</ul>

<h3>The nous vs coding_agent mystery</h3>
<p>Both used the <strong>same architecture</strong> (GDRCopy BAR1 + D2D copy). Coding_agent got 3.97 GB/s. Nous got 0.01 GB/s (140x slower).</p>
<p>Likely cause: implementation timing. Coding_agent&rsquo;s sliding-window has natural latency between NVMe write and D2D read (~22&micro;s per chunk), enough for L2 eviction. Nous may have issued D2D too soon.</p>
<p><strong>Takeaway:</strong> Architectural reasoning alone isn&rsquo;t sufficient. Success depends on subtle code-level details that hypothesis-driven agents miss but iterative optimizers stumble into.</p>

<h3>Failure modes</h3>
<table class="data-table">
<thead><tr><th>Mode</th><th>Frameworks</th><th>Description</th></tr></thead>
<tbody>
<tr><td>Success</td><td>coding_agent</td><td>3.97 GB/s (+72%) via GDRCopy BAR1 + D2D</td></tr>
<tr><td>Runtime failure</td><td>autoscientists</td><td>GDRCopy rc=22 on IPC memory; fell back to host-bounce 4.65 GB/s</td></tr>
<tr><td>Implementation failure</td><td>nous</td><td>Same architecture as coding_agent but 140x slower; timing/implementation bug</td></tr>
<tr><td>Coordination failure</td><td>gepa_native</td><td>Reproduced P2P definitions, never wired into pipeline</td></tr>
<tr><td>No attempt</td><td>ksearch, openevolve</td><td>Stuck at baseline with conservative mutations</td></tr>
</tbody></table>

<h3>Key takeaways</h3>
<ol>
<li><strong>Only 1 of 6 frameworks implemented working P2P.</strong> Explicit hints fixed discovery but not coordination or implementation.</li>
<li><strong>P2P didn&rsquo;t win on throughput for single-drive.</strong> AutoScientists&rsquo; host-bounce (4.65 GB/s) beat coding_agent&rsquo;s P2P (3.97 GB/s). P2P advantage is latency and multi-drive scalability.</li>
<li><strong>Evolutionary frameworks cannot implement architectural changes</strong> &mdash; with or without hints.</li>
<li><strong>Same design, opposite results</strong> (nous vs coding_agent) shows implementation details &gt; architectural correctness.</li>
<li><strong>Iterative hill-climbing beat hypothesis-driven</strong> for this task.</li>
</ol>
"""

FINDINGS_NO_HINT = """
<h3>Baseline</h3>
<p>2.4 GB/s, score 0.20 (1 queue x QD16, NVMe &rarr; DRAM &rarr; GPU host-bounce path)</p>

<h3>What worked</h3>
<table class="data-table">
<thead><tr><th>Framework</th><th>Score</th><th>Throughput</th><th>Approach</th></tr></thead>
<tbody>
<tr><td><strong>nous</strong></td><td>0.4808</td><td>5.96 GB/s</td><td>4 queues x QD32, 4 CUDA streams, removed mid-pipeline sync</td></tr>
<tr><td><strong>coding_agent</strong></td><td>0.3891</td><td>4.59 GB/s</td><td>Multi-object interleaving, QD36-38, 4 CUDA streams</td></tr>
<tr><td><strong>autoscientists</strong></td><td>0.3368</td><td>3.94 GB/s</td><td>Removed periodic GPU sync, 2 queues QD32</td></tr>
</tbody></table>

<h3>No framework discovered P2P independently</h3>
<ul>
<li><strong>Agentic frameworks</strong> (nous, autoscientists, coding_agent): Focused on hot-path tuning. Never explored dma.rs.</li>
<li><strong>K-Search</strong>: Correctly identified GPUDirect as highest-value action but used wrong function name in every attempt.</li>
<li><strong>OpenEvolve</strong>: Attempted P2P once with truncated function name. Failed to compile.</li>
<li><strong>Others</strong> (adaevolve, evox, shinkaevolve, gepa_native): Conservative parameter mutations only.</li>
</ul>

<h3>Why nous dominated</h3>
<p>Hypothesis-driven approach with ablation arms systematically explored the parameter space. Ablation of iter-2 proved deeper queues &gt; more threads, informing iter-3&rsquo;s winning config (4 queues x QD32 = 128 in-flight &rarr; 5.96 GB/s, near drive ceiling).</p>

<h3>Failure modes</h3>
<table class="data-table">
<thead><tr><th>Mode</th><th>Frameworks</th><th>Description</th></tr></thead>
<tbody>
<tr><td>Discovery failure</td><td>nous, autoscientists, coding_agent</td><td>Never explored dma.rs; focused on hot-path tuning</td></tr>
<tr><td>Implementation failure</td><td>ksearch, openevolve</td><td>Identified P2P but wrong function names in code</td></tr>
<tr><td>No attempt</td><td>adaevolve, evox, shinkaevolve, gepa_native</td><td>Conservative mutations only</td></tr>
</tbody></table>

<h3>Key takeaways</h3>
<ol>
<li><strong>Deep NVMe pipelining is the #1 optimization</strong> for single-drive: QD16 &rarr; QD32x4 gives 2.5x throughput with zero architectural change.</li>
<li><strong>No framework discovered P2P without explicit direction</strong>, even with nvidia-peermem/gdrdrv in hardware specs.</li>
<li><strong>Agentic frameworks vastly outperform evolutionary</strong>: nous (5.96 GB/s) vs adaevolve (2.44 GB/s).</li>
<li><strong>LLM recall failure on function names</strong> blocked K-Search and OpenEvolve despite correct reasoning.</li>
<li><strong>Local optimum traps are real</strong>: coding_agent stuck at 2 queues (4.59 GB/s); nous found 4 queues (5.96 GB/s) via ablation.</li>
</ol>
"""

CSS = """
body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    max-width: 1100px;
    margin: 0 auto;
    padding: 20px 40px;
    color: #1a1a1a;
    line-height: 1.5;
}
h1 { border-bottom: 2px solid #333; padding-bottom: 8px; }
h2 { color: #2c3e50; margin-top: 40px; border-bottom: 1px solid #ddd; padding-bottom: 4px; }
h3 { color: #34495e; margin-top: 24px; }
.data-table {
    border-collapse: collapse;
    width: 100%;
    margin: 16px 0;
    font-size: 0.9em;
}
.data-table th, .data-table td {
    border: 1px solid #ddd;
    padding: 8px 12px;
    text-align: left;
}
.data-table th { background: #f5f5f5; font-weight: 600; }
.data-table tr:nth-child(even) { background: #fafafa; }
.diagnosis { font-size: 0.85em; max-width: 400px; }
pre {
    background: #f4f4f4;
    padding: 12px;
    border-radius: 4px;
    overflow-x: auto;
    font-size: 0.9em;
}
.section { page-break-inside: avoid; }
.chart-container { margin: 20px 0; }
.toc { background: #f9f9f9; padding: 16px 24px; border-radius: 6px; margin: 20px 0; }
.toc a { text-decoration: none; color: #2c3e50; }
.toc a:hover { text-decoration: underline; }
@media print {
    body { padding: 0; }
    .chart-container { page-break-inside: avoid; }
    h2 { page-break-before: always; }
    h2:first-of-type { page-break-before: avoid; }
}
"""


def generate_report():
    # Load data for both modes
    scores_hint = load_scores(RESULTS_HINT)
    scores_no_hint = load_scores(RESULTS_NO_HINT)
    analyses_hint = load_analyses(RESULTS_HINT)
    analyses_no_hint = load_analyses(RESULTS_NO_HINT)
    summaries_hint = load_summaries(RESULTS_HINT)
    summaries_no_hint = load_summaries(RESULTS_NO_HINT)

    # Build charts
    traj_hint = make_trajectory_chart(scores_hint, "Score Trajectory — With P2P Hints")
    traj_no_hint = make_trajectory_chart(scores_no_hint, "Score Trajectory — No Hints (Control)")
    pareto_hint = make_pareto_chart(scores_hint, "Throughput vs Latency — With P2P Hints")
    pareto_no_hint = make_pareto_chart(scores_no_hint, "Throughput vs Latency — No Hints (Control)")

    # Build tables
    overview_hint = make_overview_table(scores_hint, analyses_hint, summaries_hint)
    overview_no_hint = make_overview_table(scores_no_hint, analyses_no_hint, summaries_no_hint)
    arch_hint = make_architecture_matrix(analyses_hint)
    arch_no_hint = make_architecture_matrix(analyses_no_hint)

    now = datetime.now().strftime("%Y-%m-%d %H:%M")

    html = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Evolve P2P Experiment Report</title>
<script src="https://cdn.plot.ly/plotly-2.27.0.min.js"></script>
<style>{CSS}</style>
</head>
<body>

<h1>Evolve P2P &mdash; Experiment Report</h1>
<p><em>Generated {now}</em></p>

<div class="section">
<h2 id="overview">Experiment Overview</h2>

<h3>Goal</h3>
<p>Evaluate whether AI-driven code evolution frameworks can optimize a real NVMe-to-GPU data transfer pipeline &mdash;
and specifically whether they can discover and implement GPUDirect P2P (bypassing host DRAM entirely).</p>

<h3>System Under Test</h3>
<p><strong>Certus</strong> &mdash; a generative domain-specific filesystem for GPU inference workloads.
Cold lookup path: NVMe SSD &rarr; host DRAM &rarr; GPU via cudaMemcpy. Baseline: 2.4 GB/s, ~1.9ms p99 latency.</p>

<h3>Hardware</h3>
<ul>
<li>Intel P5800X NVMe Gen4 SSD (5.9 GB/s sequential read at QD64)</li>
<li>NVIDIA A30 GPU, PCIe Gen4 x16</li>
<li>Kernel modules: nvidia-peermem, gdrdrv (GDRCopy)</li>
<li>SPDK userspace NVMe driver, VFIO-PCI, 2048 hugepages</li>
</ul>

<h3>Two Experiments</h3>
<table class="data-table">
<thead><tr><th>Experiment</th><th>Question</th><th>Frameworks</th><th>Direction Given</th></tr></thead>
<tbody>
<tr>
<td><strong>No Hints (Control)</strong></td>
<td>Can frameworks independently discover and implement architectural optimizations (including P2P)?</td>
<td>10 (3 agentic + 7 evolutionary)</td>
<td>Hardware specs only. No mention of what to optimize.</td>
</tr>
<tr>
<td><strong>With P2P Hints</strong></td>
<td>Given explicit P2P direction + FFI signatures, can frameworks implement GPUDirect Storage?</td>
<td>6 (3 agentic + 3 evolutionary)</td>
<td>Explicit: &ldquo;Implement GPUDirect Storage via GDRCopy BAR1 staging ring.&rdquo;</td>
</tr>
</tbody></table>

<h3>Scoring</h3>
<p><code>score = 0.60 &times; (throughput_gbps / 12.0) + 0.40 &times; (0.4ms / p99_latency_ms)</code></p>
<p>Baseline score: 0.20. Hardware ceiling (single drive): ~0.49.</p>
</div>

<div class="toc">
<strong>Contents</strong><br>
<a href="#nohint-findings">1. Key Findings (No Hints &mdash; Control)</a><br>
<a href="#nohint-overview">2. Overview Table (No Hints)</a><br>
<a href="#nohint-trajectory">3. Score Trajectories (No Hints)</a><br>
<a href="#nohint-pareto">4. Throughput vs Latency (No Hints)</a><br>
<a href="#nohint-arch">5. Architecture Matrix (No Hints)</a><br>
<a href="#hint-findings">6. Key Findings (With P2P Hints)</a><br>
<a href="#hint-overview">7. Overview Table (With Hints)</a><br>
<a href="#hint-trajectory">8. Score Trajectories (With Hints)</a><br>
<a href="#hint-pareto">9. Throughput vs Latency (With Hints)</a><br>
<a href="#hint-arch">10. Architecture Matrix (With Hints)</a><br>
</div>

<!-- ============================================================ -->
<!-- NO HINTS (CONTROL) — presented first -->
<!-- ============================================================ -->

<h2 id="nohint-findings">1. Key Findings &mdash; No Hints (Control)</h2>
<div class="section">
{FINDINGS_NO_HINT}
</div>

<h2 id="nohint-overview">2. Overview &mdash; No Hints</h2>
<div class="section">
{overview_no_hint}
</div>

<h2 id="nohint-trajectory">3. Score Trajectories &mdash; No Hints</h2>
<div class="chart-container">
{traj_no_hint}
</div>

<h2 id="nohint-pareto">4. Throughput vs Latency &mdash; No Hints</h2>
<div class="chart-container">
{pareto_no_hint}
</div>

<h2 id="nohint-arch">5. Architecture Matrix &mdash; No Hints</h2>
<div class="section">
{arch_no_hint}
</div>

<!-- ============================================================ -->
<!-- WITH HINTS -->
<!-- ============================================================ -->

<h2 id="hint-findings">6. Key Findings &mdash; Implement P2P (With Hints)</h2>
<div class="section">
{FINDINGS_HINT}
</div>

<h2 id="hint-overview">7. Overview &mdash; With Hints</h2>
<div class="section">
{overview_hint}
</div>

<h2 id="hint-trajectory">8. Score Trajectories &mdash; With Hints</h2>
<div class="chart-container">
{traj_hint}
</div>

<h2 id="hint-pareto">9. Throughput vs Latency &mdash; With Hints</h2>
<div class="chart-container">
{pareto_hint}
</div>

<h2 id="hint-arch">10. Architecture Matrix &mdash; With Hints</h2>
<div class="section">
{arch_hint}
</div>

</body>
</html>"""

    output = EXPERIMENT_DIR / "report.html"
    output.write_text(html)
    print(f"Report generated: {output}")
    print(f"  Hint frameworks: {len(analyses_hint)} | No-hint frameworks: {len(analyses_no_hint)}")
    print(f"  Hint evals: {len(scores_hint)} | No-hint evals: {len(scores_no_hint)}")


if __name__ == "__main__":
    generate_report()

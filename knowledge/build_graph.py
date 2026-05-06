#!/usr/bin/env python3
"""
Generates knowledge/graph.html from the actual component tree.

Organizes by dependency depth (topological order), not domain category.
Each level N depends only on levels < N.

Reads:
  - components/*/README.md                    → name, description
  - components/*/specs/*/contracts/*.md       → spec method signatures (drift detection)
  - components/interfaces/src/*.rs            → actual trait method signatures
  - knowledge/_status.md                      → known issues + missing functions
  - certus-connector/README.md                → connector status

Run: python3 knowledge/build_graph.py
"""
import os
import re

KB_ROOT = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.join(KB_ROOT, "..")
GRAPH_HTML = os.path.join(KB_ROOT, "graph.html")

DOMAIN_COLORS = {
    "Foundation": {"color": "#a78bfa", "bg": "rgba(167,139,250,0.08)"},
    "SPDK":       {"color": "#fb923c", "bg": "rgba(251,146,60,0.08)"},
    "Storage":    {"color": "#f472b6", "bg": "rgba(244,114,182,0.08)"},
    "Dispatch":      {"color": "#5eead4", "bg": "rgba(94,234,212,0.08)"},
    "GPU":        {"color": "#60a5fa", "bg": "rgba(96,165,250,0.08)"},
    "Connector":  {"color": "#4ade80", "bg": "rgba(74,222,128,0.08)"},
}

# Components ordered by dependency depth (verified from receptacle declarations)
COMPONENTS = [
    {"name": "spdk-sys",              "path": "components/spdk-sys",                "level": 0, "domain": "SPDK",       "status": "done"},
    {"name": "component-framework",   "path": "components/component-framework",     "level": 0, "domain": "Foundation", "status": "done"},
    {"name": "spdk-env",              "path": "components/spdk-env",                "level": 1, "domain": "SPDK",       "status": "done"},
    {"name": "interfaces",            "path": "components/interfaces",              "level": 1, "domain": "Foundation", "status": "done"},
    {"name": "block-device-spdk-nvme","path": "components/block-device-spdk-nvme/v2","level": 2, "domain": "SPDK",      "status": "done"},
    {"name": "gpu-services",          "path": "components/gpu-services/v0",         "level": 2, "domain": "GPU",        "status": "done"},
    {"name": "extent-manager",        "path": "components/extent-manager/v2",       "level": 3, "domain": "Storage",    "status": "done"},
    {"name": "dispatch-map",          "path": "components/dispatch-map/v0",         "level": 4, "domain": "Dispatch",      "status": "done"},
    {"name": "dispatcher",            "path": "components/dispatcher/v0",           "level": 5, "domain": "Dispatch",      "status": "needs-work"},
    {"name": "certus-connector",      "path": "certus-connector",                   "level": 6, "domain": "Connector",  "status": "in-progress"},
]

# Receptacle-wired dependencies (from define_component! declarations)
DEPENDENCIES = {
    "spdk-sys": [],
    "component-framework": [],
    "spdk-env": ["spdk-sys"],
    "interfaces": ["component-framework"],
    "block-device-spdk-nvme": ["spdk-env"],
    "gpu-services": [],
    "extent-manager": ["block-device-spdk-nvme"],
    "dispatch-map": ["extent-manager"],
    "dispatcher": ["dispatch-map", "gpu-services", "spdk-env"],
    "certus-connector": ["dispatcher", "dispatch-map", "gpu-services", "spdk-env"],
}


def compute_rdeps():
    """Compute reverse dependencies (who depends on me)."""
    rdeps = {c["name"]: [] for c in COMPONENTS}
    for name, deps in DEPENDENCIES.items():
        for dep in deps:
            if dep in rdeps:
                rdeps[dep].append(name)
    return rdeps


def read_readme(comp_path):
    """Read first paragraph from a component README."""
    readme = os.path.join(REPO_ROOT, comp_path, "README.md")
    if not os.path.exists(readme):
        return ""
    with open(readme) as f:
        lines = f.readlines()
    desc_lines = []
    in_body = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("# "):
            in_body = True
            continue
        if in_body:
            if stripped.startswith("##") or stripped.startswith("```"):
                break
            if stripped:
                desc_lines.append(stripped)
            elif desc_lines:
                break
    return " ".join(desc_lines)[:200]


def read_interface_methods(trait_file, trait_name):
    """Extract method names from a Rust trait file (only inside define_interface! block)."""
    path = os.path.join(REPO_ROOT, "components/interfaces/src", trait_file)
    if not os.path.exists(path):
        return []
    with open(path) as f:
        content = f.read()
    pattern = r'define_interface!\s*\{\s*pub\s+' + re.escape(trait_name) + r'\s*\{(.*?)\}\s*\}'
    match = re.search(pattern, content, re.DOTALL)
    if match:
        block = match.group(1)
    else:
        block = content
    methods = re.findall(r'fn\s+(\w+)\s*\(', block)
    return [m for m in methods if m not in ("fmt", "drop", "default")]


def read_spec_methods(spec_contract_path):
    """Extract method names from a spec contract .md file."""
    path = os.path.join(REPO_ROOT, spec_contract_path)
    if not os.path.exists(path):
        return []
    with open(path) as f:
        content = f.read()
    methods = re.findall(r'\|\s*`(\w+)`\s*\|', content)
    methods += re.findall(r'fn\s+(\w+)\s*\(', content)
    return list(dict.fromkeys(methods))


def detect_drift():
    """Compare spec contracts vs actual interface code."""
    drift = []

    spec_methods = read_spec_methods(
        "components/dispatch-map/v0/specs/001-dispatch-map/contracts/idispatch_map.md"
    )
    code_methods = read_interface_methods("idispatch_map.rs", "IDispatchMap")
    code_only = [m for m in code_methods if m not in spec_methods and m not in ("fmt", "drop")]
    if code_only:
        drift.append({"component": "dispatch-map", "type": "code-not-in-spec", "methods": code_only})

    spec_methods = read_spec_methods(
        "components/dispatcher/v0/specs/001-dispatcher-cache-interface/contracts/idispatcher.md"
    )
    code_methods = read_interface_methods("idispatcher.rs", "IDispatcher")
    code_only = [m for m in code_methods if m not in spec_methods and m not in ("fmt", "drop")]
    if code_only:
        drift.append({"component": "dispatcher", "type": "code-not-in-spec", "methods": code_only})

    return drift


def read_missing_functions():
    """Parse _status.md for missing function requirements."""
    status_path = os.path.join(KB_ROOT, "_status.md")
    if not os.path.exists(status_path):
        return {}
    with open(status_path) as f:
        content = f.read()

    missing = {}
    # dispatch-map missing functions
    dm_section = re.search(r'### On dispatch-map.*?\n(.*?)(?=###|\Z)', content, re.DOTALL)
    if dm_section:
        fns = re.findall(r'\|\s*`(\w+).*?\|\s*Not implemented', dm_section.group(1))
        if fns:
            missing["dispatch-map"] = fns

    # certus-connector missing wiring
    ce_section = re.search(r'### On certus-connector.*?\n(.*?)(?=###|---|\Z)', content, re.DOTALL)
    if ce_section:
        fns = re.findall(r'\|\s*`(\w+).*?\|\s*Not wired', ce_section.group(1))
        if fns:
            missing["certus-connector"] = fns

    return missing


def count_issues(comp_name):
    """Count known issues for a component from _status.md."""
    status_path = os.path.join(KB_ROOT, "_status.md")
    if not os.path.exists(status_path):
        return 0
    with open(status_path) as f:
        content = f.read()
    if comp_name == "dispatcher":
        return content.count("dispatcher `")
    return 0


def status_badge(status):
    if status == "done":
        return '<span class="badge done">done</span>'
    elif status == "in-progress":
        return '<span class="badge in-progress">in progress</span>'
    elif status == "needs-work":
        return '<span class="badge needs-work">needs work</span>'
    return '<span class="badge unknown">unknown</span>'


def domain_tag(domain):
    c = DOMAIN_COLORS.get(domain, {"color": "#9ca3b4", "bg": "rgba(156,163,180,0.08)"})
    return f'<span class="domain-tag" style="color:{c["color"]};background:{c["bg"]}">{domain}</span>'


def build_vis_nodes(drift_warnings, missing_fns):
    """Build vis-network node and edge JSON."""
    import json

    status_colors = {"done": "#4ade80", "in-progress": "#facc15", "needs-work": "#f87171"}
    domain_node_colors = {
        "Foundation": "#a78bfa", "SPDK": "#fb923c", "Storage": "#f472b6",
        "Dispatch": "#5eead4", "GPU": "#60a5fa", "Connector": "#4ade80",
    }

    nodes = []
    for comp in COMPONENTS:
        color = domain_node_colors.get(comp["domain"], "#9ca3b4")
        border_color = status_colors.get(comp["status"], "#5c6478")
        has_drift = any(d["component"] == comp["name"] for d in drift_warnings)
        has_missing = comp["name"] in missing_fns
        label = comp["name"]
        if has_drift:
            label += " ⚠"
        if has_missing:
            label += " ✦"

        title_parts = [f"<b>{comp['name']}</b>", f"Level {comp['level']} | {comp['domain']} | {comp['status']}"]
        desc = read_readme(comp["path"])
        if desc:
            title_parts.append(desc[:120])
        if has_drift:
            d = [x for x in drift_warnings if x["component"] == comp["name"]][0]
            title_parts.append(f"<b>Drift:</b> {', '.join(d['methods'])}")
        if has_missing:
            title_parts.append(f"<b>Missing:</b> {', '.join(missing_fns[comp['name']])}")
        title = "<br>".join(title_parts)

        nodes.append({
            "id": comp["name"],
            "label": label,
            "level": comp["level"],
            "color": {"background": color, "border": border_color, "highlight": {"background": color, "border": "#fff"}},
            "borderWidth": 3 if comp["status"] != "done" else 1,
            "font": {"color": "#000000", "size": 14, "face": "monospace"},
            "title": title,
            "shape": "box",
            "margin": 10,
        })

    edges = []
    for name, deps in DEPENDENCIES.items():
        for dep in deps:
            edges.append({"from": name, "to": dep, "arrows": "to", "color": {"color": "rgba(255,255,255,0.25)", "highlight": "#5eead4"}})

    return json.dumps(nodes), json.dumps(edges)


def build_html(drift_warnings, missing_fns):
    rdeps = compute_rdeps()
    nodes_json, edges_json = build_vis_nodes(drift_warnings, missing_fns)

    # Group components by level
    levels = {}
    for comp in COMPONENTS:
        levels.setdefault(comp["level"], []).append(comp)

    max_level = max(levels.keys())

    # Build level bands (top-to-bottom: highest level first)
    level_bands = []
    for lvl in range(max_level, -1, -1):
        comps = levels.get(lvl, [])
        cards = []
        for comp in comps:
            desc = read_readme(comp["path"])
            issues = count_issues(comp["name"])
            deps = DEPENDENCIES.get(comp["name"], [])
            comp_rdeps = rdeps.get(comp["name"], [])
            comp_drift = [d for d in drift_warnings if d["component"] == comp["name"]]
            comp_missing = missing_fns.get(comp["name"], [])

            deps_html = ", ".join(f'<span class="dep">{d}</span>' for d in deps) if deps else '<span class="dep none">none</span>'
            rdeps_html = ", ".join(f'<span class="rdep">{r}</span>' for r in comp_rdeps) if comp_rdeps else '<span class="dep none">none</span>'
            issue_html = f' <span class="issue-badge">{issues} bugs</span>' if issues > 0 else ""

            drift_html = ""
            if comp_drift:
                methods = comp_drift[0]["methods"]
                drift_html = f'<div class="card-drift">Drift: {", ".join(methods)} (in code, not spec)</div>'

            missing_html = ""
            if comp_missing:
                missing_html = f'<div class="card-missing">Missing: {", ".join(comp_missing)}</div>'

            cards.append(f'''<div class="comp-card">
        <div class="comp-header">
          <span class="comp-name">{comp["name"]}</span>{issue_html}
          {domain_tag(comp["domain"])}
        </div>
        <div class="comp-status">{status_badge(comp["status"])}</div>
        <div class="comp-desc">{desc}</div>
        <div class="comp-deps"><span class="dep-label">deps:</span> {deps_html}</div>
        <div class="comp-deps"><span class="dep-label">used by:</span> {rdeps_html}</div>
        {drift_html}
        {missing_html}
      </div>''')

        cards_html = "\n      ".join(cards)
        level_bands.append(f'''<div class="level-band">
    <div class="level-tag">Level {lvl}</div>
    <div class="comp-row">
      {cards_html}
    </div>
  </div>''')

    levels_section = "\n  ".join(level_bands)

    # Drift summary
    drift_html = ""
    if drift_warnings:
        items = "\n".join(
            f'    <li><strong>{d["component"]}</strong>: {", ".join(d["methods"])} (in code, not in spec)</li>'
            for d in drift_warnings
        )
        drift_html = f'''<div class="drift-section">
    <h3>Spec-vs-Code Drift</h3>
    <ul>
{items}
    </ul>
  </div>'''

    # Missing summary
    missing_html = ""
    if missing_fns:
        items = "\n".join(
            f'    <li><strong>{comp}</strong>: {", ".join(fns)}</li>'
            for comp, fns in missing_fns.items()
        )
        missing_html = f'''<div class="missing-section">
    <h3>Missing Functions (needed for vLLM contract)</h3>
    <ul>
{items}
    </ul>
  </div>'''

    # Stats
    total = len(COMPONENTS)
    done = sum(1 for c in COMPONENTS if c["status"] == "done")
    in_prog = sum(1 for c in COMPONENTS if c["status"] == "in-progress")
    needs = sum(1 for c in COMPONENTS if c["status"] == "needs-work")

    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Certus Dependency Graph</title>
<script src="https://unpkg.com/vis-network/standalone/umd/vis-network.min.js"></script>
<style>
:root{{--bg:#0c0e14;--card:#13161f;--border:rgba(255,255,255,0.06);--text-1:#e8eaf0;--text-2:#9ca3b4;--text-3:#5c6478}}
*{{box-sizing:border-box;margin:0;padding:0}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',system-ui,sans-serif;background:var(--bg);color:var(--text-1);line-height:1.6;padding:0}}
.tabs{{display:flex;border-bottom:1px solid var(--border);background:var(--card)}}
.tab{{padding:12px 24px;cursor:pointer;font-size:13px;font-weight:600;color:var(--text-3);border-bottom:2px solid transparent}}
.tab.active{{color:var(--text-1);border-bottom-color:#5eead4}}
.tab:hover{{color:var(--text-2)}}
.tab-content{{display:none}}
.tab-content.active{{display:block}}
#graph-view{{width:100%;height:calc(100vh - 44px)}}
#card-view{{padding:24px;max-width:1000px;margin:0 auto;max-height:calc(100vh - 44px);overflow-y:auto}}
h1{{font-size:24px;font-weight:700;margin:0 0 4px}}
h1 .hl{{background:linear-gradient(135deg,#5eead4,#818cf8);-webkit-background-clip:text;-webkit-text-fill-color:transparent}}
.subtitle{{color:var(--text-3);font-size:13px;margin:0 0 6px}}
.purpose{{color:var(--text-2);font-size:12px;margin:0 0 20px;line-height:1.7}}
.purpose strong{{color:var(--text-1)}}
.stats{{display:flex;gap:12px;margin:0 0 20px;flex-wrap:wrap}}
.stat{{background:var(--card);border:1px solid var(--border);border-radius:8px;padding:8px 14px;font-size:12px;color:var(--text-3)}}
.stat strong{{color:var(--text-1);font-size:18px;display:block}}
.level-band{{border-radius:10px;padding:14px 18px;margin:0 0 8px;border:1px solid var(--border);background:var(--card)}}
.level-tag{{display:inline-block;font-size:10px;font-weight:700;text-transform:uppercase;letter-spacing:0.1em;color:var(--text-3);padding:2px 8px;border-radius:4px;background:rgba(255,255,255,0.03);margin:0 0 10px}}
.comp-row{{display:flex;gap:10px;flex-wrap:wrap}}
.comp-card{{background:rgba(255,255,255,0.02);border:1px solid var(--border);border-radius:8px;padding:12px 14px;flex:1;min-width:260px}}
.comp-header{{display:flex;align-items:center;gap:8px;margin:0 0 4px;flex-wrap:wrap}}
.comp-name{{font-size:14px;font-weight:700}}
.domain-tag{{font-size:9px;font-weight:700;text-transform:uppercase;letter-spacing:0.06em;padding:2px 6px;border-radius:4px}}
.comp-status{{margin:0 0 6px}}
.badge{{font-size:11px;font-weight:700;padding:1px 6px;border-radius:3px}}
.badge.done{{color:#4ade80;background:rgba(74,222,128,0.1)}}
.badge.in-progress{{color:#facc15;background:rgba(250,204,21,0.1)}}
.badge.needs-work{{color:#f87171;background:rgba(248,113,113,0.1)}}
.comp-desc{{font-size:11px;color:var(--text-3);margin:0 0 8px;line-height:1.5}}
.comp-deps{{font-size:10px;color:var(--text-3);margin:2px 0}}
.dep-label{{color:var(--text-3);font-weight:600}}
.dep,.rdep{{display:inline-block;background:rgba(255,255,255,0.05);padding:1px 6px;border-radius:3px;margin:0 2px;font-family:monospace;font-size:10px}}
.rdep{{background:rgba(94,234,212,0.08);color:#5eead4}}
.dep.none{{color:var(--text-3);background:none}}
.issue-badge{{font-size:10px;color:#f87171;font-weight:700}}
.card-drift{{font-size:10px;color:#fb923c;margin:6px 0 0;padding:4px 8px;background:rgba(251,146,60,0.06);border-radius:4px}}
.card-missing{{font-size:10px;color:#f472b6;margin:4px 0 0;padding:4px 8px;background:rgba(244,114,182,0.06);border-radius:4px}}
.drift-section,.missing-section{{border-radius:10px;padding:14px 18px;margin:0 0 16px}}
.drift-section{{background:rgba(251,146,60,0.04);border:1px solid rgba(251,146,60,0.15)}}
.drift-section h3{{font-size:13px;color:#fb923c;margin:0 0 6px}}
.missing-section{{background:rgba(244,114,182,0.04);border:1px solid rgba(244,114,182,0.15)}}
.missing-section h3{{font-size:13px;color:#f472b6;margin:0 0 6px}}
.drift-section ul,.missing-section ul{{margin:0 0 0 16px;font-size:11px;color:var(--text-2)}}
.drift-section li,.missing-section li{{margin:3px 0}}
footer{{padding:16px 0;text-align:center;font-size:11px;color:var(--text-3);margin-top:20px;border-top:1px solid var(--border)}}
footer code{{font-family:monospace;font-size:10px;background:rgba(255,255,255,0.06);padding:2px 5px;border-radius:3px}}
#sidebar{{position:fixed;right:0;top:44px;width:280px;height:calc(100vh - 44px);background:#1a1a2e;border-left:1px solid #2a2a4e;padding:16px;overflow-y:auto;display:none;z-index:10}}
#sidebar.open{{display:block}}
#sidebar h3{{font-size:12px;color:#aaa;text-transform:uppercase;letter-spacing:0.05em;margin:0 0 10px}}
#sidebar .field{{margin:6px 0;font-size:12px;color:#ccc}}
#sidebar .field b{{color:#e8eaf0}}
#sidebar .close-btn{{position:absolute;top:10px;right:14px;cursor:pointer;color:#666;font-size:18px}}
#sidebar .close-btn:hover{{color:#fff}}
#legend{{position:fixed;left:16px;bottom:16px;background:#1a1a2e;border:1px solid #2a2a4e;border-radius:8px;padding:12px 16px;z-index:10;font-size:11px}}
#legend h4{{color:#aaa;font-size:10px;text-transform:uppercase;letter-spacing:0.08em;margin:0 0 8px}}
.legend-row{{display:flex;align-items:center;gap:8px;margin:4px 0}}
.legend-dot{{width:12px;height:12px;border-radius:3px;flex-shrink:0}}
.legend-label{{color:#ccc}}
</style>
</head>
<body>

<div class="tabs">
  <div class="tab active" onclick="switchTab('graph')">Graph</div>
  <div class="tab" onclick="switchTab('cards')">Cards</div>
</div>

<div id="graph-tab" class="tab-content active">
  <div id="graph-view"></div>
  <div id="legend">
    <h4>Domains</h4>
    <div class="legend-row"><div class="legend-dot" style="background:#a78bfa"></div><span class="legend-label">Foundation</span></div>
    <div class="legend-row"><div class="legend-dot" style="background:#fb923c"></div><span class="legend-label">SPDK</span></div>
    <div class="legend-row"><div class="legend-dot" style="background:#f472b6"></div><span class="legend-label">Storage</span></div>
    <div class="legend-row"><div class="legend-dot" style="background:#5eead4"></div><span class="legend-label">Dispatch</span></div>
    <div class="legend-row"><div class="legend-dot" style="background:#60a5fa"></div><span class="legend-label">GPU</span></div>
    <div class="legend-row"><div class="legend-dot" style="background:#4ade80"></div><span class="legend-label">Connector</span></div>
    <h4 style="margin-top:10px">Status (border)</h4>
    <div class="legend-row"><div class="legend-dot" style="background:transparent;border:2px solid #4ade80"></div><span class="legend-label">done</span></div>
    <div class="legend-row"><div class="legend-dot" style="background:transparent;border:2px solid #facc15"></div><span class="legend-label">in progress</span></div>
    <div class="legend-row"><div class="legend-dot" style="background:transparent;border:2px solid #f87171"></div><span class="legend-label">needs work</span></div>
  </div>
</div>

<div id="sidebar">
  <span class="close-btn" onclick="this.parentElement.classList.remove('open')">&times;</span>
  <h3>Component Info</h3>
  <div id="sidebar-content"></div>
</div>

<div id="cards-tab" class="tab-content">
  <div id="card-view">
    <h1><span class="hl">Certus</span> Dependency Graph</h1>
    <p class="subtitle">Organized by dependency depth &mdash; level N depends only on levels &lt; N</p>
    <p class="purpose">
      <strong>Where am I?</strong> Level N in the DAG &nbsp;|&nbsp;
      <strong>What can I touch?</strong> Levels below = your deps; above = depend on you &nbsp;|&nbsp;
      <strong>What's broken?</strong> Red badges + drift &nbsp;|&nbsp;
      <strong>What's missing?</strong> Pink cards
    </p>

    <div class="stats">
      <div class="stat"><strong>{total}</strong>components</div>
      <div class="stat"><strong style="color:#4ade80">{done}</strong>done</div>
      <div class="stat"><strong style="color:#facc15">{in_prog}</strong>in progress</div>
      <div class="stat"><strong style="color:#f87171">{needs}</strong>needs work</div>
      <div class="stat"><strong>{max_level + 1}</strong>depth levels</div>
    </div>

    {drift_html}
    {missing_html}

    {levels_section}

    <footer>Generated from component tree &middot; <code>python3 knowledge/build_graph.py</code></footer>
  </div>
</div>

<script>
function switchTab(name) {{
  document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
  document.querySelectorAll('.tab-content').forEach(t => t.classList.remove('active'));
  if (name === 'graph') {{
    document.querySelectorAll('.tab')[0].classList.add('active');
    document.getElementById('graph-tab').classList.add('active');
  }} else {{
    document.querySelectorAll('.tab')[1].classList.add('active');
    document.getElementById('cards-tab').classList.add('active');
  }}
}}

var nodes = new vis.DataSet({nodes_json});
var edges = new vis.DataSet({edges_json});

var container = document.getElementById('graph-view');
var data = {{ nodes: nodes, edges: edges }};
var options = {{
  layout: {{
    hierarchical: {{
      direction: 'DU',
      sortMethod: 'directed',
      levelSeparation: 120,
      nodeSpacing: 180,
      blockShifting: true,
      edgeMinimization: true
    }}
  }},
  physics: false,
  interaction: {{
    hover: true,
    tooltipDelay: 100,
    zoomView: true,
    dragView: true
  }},
  edges: {{
    smooth: {{ type: 'cubicBezier', forceDirection: 'vertical' }},
    width: 1.5,
    selectionWidth: 3
  }},
  nodes: {{
    borderWidth: 1,
    borderWidthSelected: 3,
    shadow: {{ enabled: true, size: 8, color: 'rgba(0,0,0,0.3)' }}
  }}
}};

var network = new vis.Network(container, data, options);

network.on('click', function(params) {{
  if (params.nodes.length > 0) {{
    var nodeId = params.nodes[0];
    var node = nodes.get(nodeId);
    var sidebar = document.getElementById('sidebar');
    var content = document.getElementById('sidebar-content');
    content.innerHTML = node.title || nodeId;
    sidebar.classList.add('open');
  }}
}});
</script>
</body>
</html>"""


def run():
    drift = detect_drift()
    missing = read_missing_functions()
    html = build_html(drift, missing)

    with open(GRAPH_HTML, "w") as f:
        f.write(html)

    total = len(COMPONENTS)
    done = sum(1 for c in COMPONENTS if c["status"] == "done")
    print(f"Graph generated: {total} components ({done} done), 7 depth levels")
    if drift:
        print(f"  Drift warnings: {len(drift)}")
        for d in drift:
            print(f"    - {d['component']}: {', '.join(d['methods'])}")
    if missing:
        print(f"  Missing functions:")
        for comp, fns in missing.items():
            print(f"    - {comp}: {', '.join(fns)}")
    print(f"  Output: {GRAPH_HTML}")


if __name__ == "__main__":
    run()

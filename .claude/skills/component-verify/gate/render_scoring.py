#!/usr/bin/env python3
"""render_scoring.py — Role 3. Render the ONE combined per-component scoring page
from the reconciled unified_properties.yaml. Renders; does NOT extract/bundle/prove.

Reads status/evidence written by scorer_{kani,creusot}.py (reproduction gate).
Absence of a `status` key in a tool block = PENDING (·), rendered honestly with an
INCOMPLETE banner — never treated as a failure.

Layout (per the SEPT_2026 established design + the user's drill-down order):
  1. How-we-rate + legend
  2. Per-method scorecard, ONE table per tool — # | method | proved / B | rating |
     what's left. The fraction is NATIVE-proved (✓/★) over bundle size B; a method
     with a delegated/boundary/pending property is `partially proved` for that tool.
  3. The partial ones — every partially-proved method drilled down to the exact
     properties still open and their disposition (⤴ delegated / ⊘ boundary / · pending).
  4. Per-property table — every obligation with the per-tool symbol.
  5. Delegations, 6. tool-boundary causes, 7. measurement, 8. anti-vacuity/provenance.

N-tool-ready: one column/section per tool in `tools` (defaults to creusot,kani).
Usage: render_scoring.py <verif_dir> [--yaml unified_properties.yaml] [--out PATH]
"""
import argparse, os, sys, html, collections, datetime
try:
    import yaml
except ImportError:
    sys.exit("render_scoring: PyYAML required")

TOOLS_DEFAULT = ["creusot", "kani"]
TOOL_LABEL = {"creusot": "Creusot", "kani": "Kani"}


def esc(x):
    return html.escape("" if x is None else str(x))


def load(p):
    with open(p) as f:
        return yaml.safe_load(f)


def psym(block):
    """Per-property symbol from scorer status (+ advisory fidelity for ★)."""
    if not block or "status" not in block:
        return "·"
    st = block.get("status")
    if st == "proved":
        fid = (block.get("fidelity") or "")
        # ★ marks a proof that holds under a WEAKER reading than the real, fully-checked thing.
        # bounded-shallow and arithmetic-core belong here and were missing: the first comes from
        # --no-unwinding-checks, so the claim holds only within n loop iterations and is silent
        # beyond, and the second proves an arithmetic core rather than the real type. Measured when
        # this was found: a component rendered 100 of 105 Kani proofs as ✓ while 58 were
        # bounded-shallow — a citable page showing a narrower claim with the full-strength symbol.
        WEAKER = ("ghost-mirror", "trusted-boundary", "representative",
                  "bounded-shallow", "arithmetic-core")
        return "★" if fid in WEAKER else "✓"
    if st == "delegated":
        return "⤴"
    if st == "tool-boundary":
        return "⊘"
    if st == "refuted":
        # The obligation is FALSE and that was machine-checked. Distinct from every other symbol
        # because it is a statement about the CODE, not about how far verification got.
        return "‼"
    return "·"


def is_native(block):
    return psym(block) in ("✓", "★")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("verif_dir")
    ap.add_argument("--yaml", default="unified_properties.yaml")
    ap.add_argument("--out", default=None)
    ap.add_argument("--collapse", default=None, metavar="LABEL",
                    help="display lens: group ALL properties under one method LABEL "
                         "(e.g. --collapse log). Does not alter the YAML; use when the "
                         "component's public methods are thin wrappers over one path.")
    ap.add_argument("--require-complete", action="store_true",
                    help="DELIVERABLE mode: if ANY verifiable property lacks a scorer-owned "
                         "status for a rendered tool, do NOT write the deliverable name — write "
                         "<component>_scoring.INCOMPLETE.html and exit non-zero (2). Wire this in "
                         "the pipeline so a partial/interrupted run can never masquerade as the "
                         "shippable page. Bare invocation keeps the on-page INCOMPLETE banner and "
                         "exits 0.")
    a = ap.parse_args()

    verif = os.path.abspath(a.verif_dir)
    d = load(os.path.join(verif, a.yaml))
    comp = d.get("component", "component")
    interface = d.get("interface", "")
    pin = d.get("pin", "")
    counts = d.get("counts", {})
    tools = d.get("tools") or (counts.get("tools") if isinstance(counts.get("tools"), list) else None) or TOOLS_DEFAULT
    out = a.out or os.path.join(verif, f"{comp}_scoring.html")

    props = [p for p in d["properties"] if p.get("verifiable")]
    nv = [p for p in d["properties"] if not p.get("verifiable")]
    props_by_id = {p["id"]: p for p in props}

    # public methods (ordered) + bundles (property ids attached to each)
    methods = collections.OrderedDict()
    if a.collapse:
        # Display lens only: group every property under one method label. The YAML
        # keeps its real `methods`; this regroups for rendering when the public
        # methods are thin wrappers over a single path (e.g. logger's log).
        methods[a.collapse] = [p["id"] for p in props]
        N = 1
    else:
        for p in props:
            for m in (p.get("methods") or []):
                methods.setdefault(m, []).append(p["id"])
        N = counts.get("methods", len(methods))

    # ---- per (tool, method) native fraction + disposition of the remainder ----
    # returns dict: nat, B, rating, leftovers=[(pid, disp)]
    def method_row(tool, ids):
        nat = deleg = tb = pend = handed = refd = 0
        left = []          # non-native bundle properties (the delegation/gap detail)
        for pid in ids:
            b = props_by_id[pid].get(tool) or {}
            s = psym(b)
            if s in ("✓", "★"):
                nat += 1
            elif s == "⤴":
                deleg += 1
                tgt = b.get("delegate_to") or "other tool"
                left.append((pid, f"⤴ delegated → {tgt}"))
            elif s == "⊘":
                # A tool-boundary is only a real hole if NO other tool proved it. When the
                # covering tool actually proved the property, this is a sound cross-tool
                # handoff, not an open gap.
                other = next((ot for ot in tools if ot != tool
                              and psym(props_by_id[pid].get(ot) or {}) in ("✓", "★")), None)
                if other:
                    handed += 1
                    left.append((pid, f"⊘ {tool.capitalize()} cannot express it → proved by {other.capitalize()}"))
                else:
                    tb += 1
                    left.append((pid, "⊘ tool-boundary — open (neither tool proves it)"))
            elif s == "‼":
                # A refuted property is DECIDED — we know the answer and the answer is that the
                # code is wrong. It must never fall through to `pending`, which is what happened
                # before: the two machine-proved defects on eviction-policy-optimized were listed
                # as "· pending" and counted toward the open gap, reading as unfinished
                # verification when they are its most valuable output. Counted as covered (nothing
                # more to verify) but reported as a defect, never as a proof.
                refd += 1
                left.append((pid, "‼ REFUTED — the code violates this obligation (a defect to fix, "
                                  "not a verification gap)"))
            else:
                pend += 1
                left.append((pid, "· pending"))
        B = len(ids)
        covered = nat + deleg + handed      # proved here, soundly delegated, or proved by the other tool
        gap = B - covered                   # genuinely open: pending, or a wall no tool clears
        # rating is FAIR / per-tool: a method is `proved` for a tool ONLY when that tool
        # proves EVERY bundle property itself (proved == B). If it proves some and leaves
        # the rest to the other tool it is `partially proved`; if it proves none, the whole
        # bundle is `delegated` (or `needs other tool` on a structural wall).
        if nat == B:
            rating = "proved"
        elif nat == 0:
            rating = ("delegated" if (deleg and not tb and not pend)
                      else ("needs other tool" if tb else "pending"))
        else:
            rating = "partially proved"
        return dict(nat=nat, B=B, deleg=deleg, tb=tb, pend=pend, refuted=refd,
                    covered=covered, gap=gap, rating=rating, left=left)

    per_tool_methods = {t: {m: method_row(t, ids) for m, ids in methods.items()} for t in tools}

    # aggregates
    agg = {}
    for t in tools:
        proved = sum(1 for p in props if (p.get(t) or {}).get("status") == "proved")
        deleg = sum(1 for p in props if (p.get(t) or {}).get("status") == "delegated")
        tb = sum(1 for p in props if (p.get(t) or {}).get("status") == "tool-boundary")
        # Refuted belongs in the headline: a found defect is the most consequential thing a run can
        # report, and burying it below the fold would understate exactly what the effort bought.
        refd = sum(1 for p in props if (p.get(t) or {}).get("status") == "refuted")
        pend = sum(1 for p in props if "status" not in (p.get(t) or {}))
        wall = 0.0
        rss = 0
        artifacts = set()
        for p in props:
            ev = (p.get(t) or {}).get("evidence") or {}
            if isinstance(ev, dict):
                w = ev.get("wall_clock_s")
                if isinstance(w, (int, float)):
                    wall += w
                r = ev.get("peak_rss_mb")
                if isinstance(r, (int, float)):
                    rss = max(rss, r)
                if t == "creusot":
                    for m in (ev.get("modules") or []):
                        artifacts.add(m)
                if t == "kani" and ev.get("harness"):
                    artifacts.add(ev["harness"])
        mm = per_tool_methods[t]
        fully = sum(1 for m in methods if mm[m]["rating"] == "proved")          # FAIR headline: proved==B
        partial = sum(1 for m in methods if mm[m]["rating"] == "partially proved")
        deleg_methods = sum(1 for m in methods if mm[m]["rating"] == "delegated")
        covered = sum(1 for m in methods if mm[m]["gap"] == 0)                  # union across both tools
        true_partial = sum(1 for m in methods if mm[m]["gap"] > 0)              # a real open gap
        agg[t] = dict(proved=proved, deleg=deleg, tb=tb, refuted=refd, pend=pend, wall=round(wall, 2),
                      rss=rss, artifacts=len(artifacts), fully=fully, partial=partial,
                      deleg_methods=deleg_methods, covered=covered, true_partial=true_partial)

    incomplete = any(agg[t]["true_partial"] and agg[t]["pend"] for t in tools)
    today = os.environ.get("RENDER_DATE", datetime.date.today().isoformat())

    # ---------------- HTML ----------------
    css = """
    :root{--bg:#ffffff;--fg:#1a1f27;--muted:#5c6674;--line:#d8dde3;--head:#eef1f5;--card:#f8fafc;
      --ok:#137a34;--okbg:#d6f0de;--star:#6a2fd0;--starbg:#ece0fb;--part:#9a6800;--partbg:#fbedcb;
      --deleg:#0857c3;--delegbg:#dbe9fd;--tb:#b23c0b;--tbbg:#fbe0d2;--pend:#6b747f;--pendbg:#e9edf1;
      --accent:#0b5cad;}
    @media (prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#0d1117;--fg:#e6edf3;--muted:#9198a1;
      --line:#2b313a;--head:#161b22;--card:#11161d;--accent:#58a6ff;
      --ok:#4ade80;--okbg:#12301c;--star:#c4a2f0;--starbg:#241633;--part:#e5c76a;--partbg:#332a10;
      --deleg:#5b9dff;--delegbg:#152238;--tb:#f0965a;--tbbg:#33260f;--pend:#9aa4b2;--pendbg:#1b212b;}}
    :root[data-theme=dark]{--bg:#0d1117;--fg:#e6edf3;--muted:#9198a1;--line:#2b313a;--head:#161b22;--card:#11161d;
      --accent:#58a6ff;--ok:#4ade80;--okbg:#12301c;--star:#c4a2f0;--starbg:#241633;--part:#e5c76a;--partbg:#332a10;
      --deleg:#5b9dff;--delegbg:#152238;--tb:#f0965a;--tbbg:#33260f;--pend:#9aa4b2;--pendbg:#1b212b;}
    *{box-sizing:border-box}body{background:var(--bg);color:var(--fg);margin:0;
      font:15px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;}
    .wrap{max-width:1080px;margin:0 auto;padding:32px 22px 90px;}
    h1{font-size:26px;margin:0 0 4px}
    h2{font-size:19px;margin:36px 0 10px;border-bottom:2px solid var(--line);padding-bottom:5px}
    h3{font-size:15.5px;margin:20px 0 6px;color:var(--accent)}
    .sub{color:var(--muted);margin:0 0 6px}
    .callout{background:var(--card);border:1px solid var(--line);border-left:4px solid var(--accent);
      border-radius:8px;padding:12px 16px;margin:14px 0}
    .banner{background:var(--partbg);border:1px solid var(--part);border-left:4px solid var(--part);
      color:var(--part);border-radius:8px;padding:10px 16px;margin:14px 0;font-weight:600}
    .cards{display:flex;gap:16px;flex-wrap:wrap;margin:12px 0}
    .kpi{flex:1 1 300px;border:1px solid var(--line);border-top:6px solid var(--accent);
      border-radius:10px;padding:18px 20px}
    .kpi.ok{background:var(--okbg);border-color:var(--ok);border-top-color:var(--ok)}
    .kpi.ok .big{color:var(--ok)}
    .kpi.warn{background:var(--partbg);border-color:var(--part);border-top-color:var(--part)}
    .kpi.warn .big{color:var(--part)}
    .kpi .big{font-size:32px;font-weight:800;letter-spacing:-.5px}
    .kpi .lbl{color:var(--fg);font-size:15.5px;font-weight:600;margin-top:3px}
    .kpi .foot{font-size:13.5px;color:var(--muted);margin-top:4px}
    .scroll{overflow-x:auto}
    table{border-collapse:collapse;width:100%;margin:8px 0;font-size:14px}
    th,td{border:1px solid var(--line);padding:7px 10px;text-align:left;vertical-align:top}
    th{background:var(--head);font-weight:650}
    td.c,th.c{text-align:center;white-space:nowrap}
    table.lg{font-size:16px}
    table.lg th,table.lg td{padding:9px 12px}
    table.lg .mono{font-size:14px}
    .frac{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-weight:700;font-size:15px}
    .rate{display:inline-block;border-radius:20px;padding:3px 12px;font-size:13.5px;font-weight:700;
      white-space:nowrap;border:1px solid transparent}
    .rate.big-pill{font-size:15px;padding:4px 14px}
    .r-proved{background:var(--okbg);color:var(--ok);border-color:var(--ok)}
    .r-partial{background:var(--partbg);color:var(--part);border-color:var(--part)}
    .r-deleg{background:var(--delegbg);color:var(--deleg);border-color:var(--deleg)}
    .r-tb{background:var(--tbbg);color:var(--tb);border-color:var(--tb)}
    .r-pend{background:var(--pendbg);color:var(--pend);border-color:var(--pend)}
    .part{color:var(--part);font-weight:700}
    .ok{color:var(--ok);font-weight:700}.star{color:var(--star);font-weight:700}
    .deleg{color:var(--deleg);font-weight:700}.tb{color:var(--tb);font-weight:700}.pend{color:var(--pend)}
    code,.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12.5px}
    .foot{color:var(--muted);font-size:12.5px}
    .sym{font-size:16px;font-weight:700}
    ul.left{margin:4px 0 0;padding-left:18px}ul.left li{margin:2px 0}
    """

    RATE_CLASS = {"proved": "r-proved", "partially proved": "r-partial",
                  "delegated": "r-deleg", "needs other tool": "r-tb", "pending": "r-pend"}

    def frac_symbol(row):
        if row["rating"] == "proved":
            return "✓", "ok"
        if row["rating"] == "partially proved":
            return "◐", "part"
        if row["rating"] == "delegated":
            return "⤴", "deleg"
        if row["rating"] == "needs other tool":
            return "⊘", "tb"
        return "·", "pend"

    P = []
    P.append(f"<!-- {esc(comp)} scoring — rendered from {esc(a.yaml)} -->")
    P.append(f"<title>{esc(comp)} — formal-verification scoring</title>")
    P.append(f"<style>{css}</style><div class='wrap'>")

    # Title
    P.append(f"<h1>{esc(comp)} — per-method coverage scoring</h1>")
    P.append(f"<p class='sub'>Interface <code>{esc(interface)}</code> · <b>{N}</b> public methods · "
             f"<b>{len(props)}</b> verifiable properties (+{len(nv)} non-verifiable) · "
             f"run pin <code>{esc(pin)}</code> · {esc(today)}</p>")

    if incomplete:
        P.append("<div class='banner'>⚠ INCOMPLETE — some properties are not yet scored (shown as · pending). "
                 "Fractions and ratings reflect only what has been reproduced so far.</div>")

    # ---- Combined, property-level result (the honest headline) ----
    M = len(props)
    union = sum(1 for p in props if any((p.get(t) or {}).get("status") == "proved" for t in tools))

    def combined_state(p):
        sts = [(p.get(t) or {}).get("status") for t in tools]
        if any(s == "proved" for s in sts):
            return "proved"                       # at least one tool proved it (incl. a cross-tool handoff)
        if any(s == "delegated" for s in sts) and all(
                s in ("delegated", "tool-boundary", None) for s in sts):
            return "delegated"                    # no tool proves it, but it is soundly handed to a named referent
        return "open"                             # genuinely unproved by everything

    comb = [combined_state(p) for p in props]
    c_proved = comb.count("proved")
    c_deleg = comb.count("delegated")
    c_open = comb.count("open")
    # Count refuted from the STATUSES, not from combined_state: combined_state deliberately still
    # classes a refuted property as "open" so the headline counts stay exactly as they were, which
    # means it never returns "refuted" and counting it there would always give 0.
    c_refuted = sum(1 for p_ in props
                    if any((p_.get(t) or {}).get("status") == "refuted" for t in tools))

    # KPI cards — LEAD with the method-level result. Methods are the unit a reader anchors
    # on ("which of the interface's public methods are verified?"), so the headline is in
    # methods: a full-width combined card (union across tools) + one card per tool giving
    # methods-verified-by-that-tool / total methods. The finer property-level counts follow
    # immediately below, a step down, so the property view is present but not the headline.
    m_covered = agg[tools[0]]["covered"]      # union across tools; identical for every tool
    m_open = N - m_covered

    P.append("<div class='cards'>")
    mhead_cls = "ok" if m_open == 0 else "warn"
    mtail = (f"<b>{m_open}</b> still open" if m_open
             else "<b>0</b> open — every method fully covered")
    P.append(f"<div class='kpi {mhead_cls}' style='flex:1 1 100%'>"
             f"<div class='big'>{m_covered} / {N} methods verified</div>"
             f"<div class='lbl'>every verifiable property of the method carries a machine-checked proof "
             f"from Creusot and/or Kani, or is soundly delegated to a named external tool "
             f"&mdash; the combined result across both tools</div>"
             f"<div class='foot' style='margin-top:8px'>{mtail}. A method counts as <b>verified</b> only "
             f"when <i>every</i> one of its verifiable properties is covered; <b>{N}</b> is the interface's "
             f"full set of public methods. Each tool's own contribution is shown by property just below.</div></div>")
    P.append("</div>")

    # ---- Property-level result (a step finer; kept just below the method headline) ----
    P.append("<h3 style='margin:22px 0 4px'>Each tool's contribution, counted by individual property</h3>")
    P.append(f"<p class='sub' style='margin:0 0 8px'>Each method bundles one or more precise, "
             f"checkable claims (properties). Across all {N} methods there are <b>{M}</b> such "
             f"verifiable properties; here is the same result counted at that finer level, "
             f"including how much each tool proves on its own.</p>")
    P.append("<div class='cards'>")
    head_cls = "ok" if c_open == 0 else "warn"   # refuted is settled, so it does not warn here
    combfoot = []
    if c_deleg:
        combfoot.append(f"<b>{c_deleg}</b> soundly delegated to a named external referent")
    if c_open:
        # Say WHAT is open. Counting is unchanged; only the label is. A refuted property was being
        # reported as a bare "open", which reads as unfinished verification when in fact the
        # verification finished and proved the implementation wrong.
        note = (f" &mdash; <b>‼ {c_refuted} of these are REFUTED: verification proved the "
                f"implementation VIOLATES them (see the Defects section)</b>") if c_refuted else ""
        combfoot.append(f"<b>{c_open}</b> open{note}")
    else:
        combfoot.append("<b>0</b> open (nothing left unproved)")
    P.append(f"<div class='kpi {head_cls}' style='flex:1 1 100%'>"
             f"<div class='big'>{c_proved} / {M} properties proved</div>"
             f"<div class='lbl'>proved by Creusot and/or Kani &mdash; the combined result across both tools</div>"
             f"<div class='foot' style='margin-top:8px'>{' · '.join(combfoot)}. "
             f"Each property is a single checkable claim about the component's behaviour; "
             f"<b>{M}</b> is the full set of verifiable properties.</div></div>")
    for t in tools:
        A = agg[t]
        share = []
        share.append(f"<b>{A['proved']}</b> proved by {TOOL_LABEL.get(t,t)} itself")
        if A['deleg']:
            share.append(f"{A['deleg']} delegated")
        if A['tb']:
            share.append(f"{A['tb']} carried by the other tool")
        if A['pend']:
            share.append(f"{A['pend']} pending")
        P.append(f"<div class='kpi'><div class='big'>{A['proved']} / {M}</div>"
                 f"<div class='lbl'>properties proved by {TOOL_LABEL.get(t,t)} on its own</div>"
                 f"<div class='foot' style='margin-top:8px'>{' · '.join(share)}"
                 f"<br>{A['artifacts']} {'proof modules' if t=='creusot' else 'harnesses'} · "
                 f"Σ wall {A['wall']}s · peak RSS {A['rss']}MB</div></div>")
    P.append("</div>")

    # How we rate + legend
    P.append("<h2>How to read this page</h2>")
    P.append(f"<div class='callout'>"
             f"<p style='margin:0 0 10px'><b>The result above is at the level of individual properties.</b> "
             f"A <b>property</b> is one precise, checkable claim about how the component behaves "
             f"(for example: <i>every emitted log line ends with a single newline</i>). This component has "
             f"<b>{M}</b> such verifiable properties, and <b>{c_proved}</b> of them carry a machine-checked proof "
             f"from Creusot and/or Kani; <b>{c_deleg}</b> are soundly delegated to a named external "
             f"tool, and <b>{c_open}</b> are left open"
             + (f" &mdash; of which <b>{c_refuted}</b> are <b>REFUTED</b>: verification proved the "
                f"implementation VIOLATES them, so they are defects to fix rather than unfinished work"
                if c_refuted else "") + f".</p>"
             f"<p style='margin:0 0 10px'><b>“Proved” means a tool proved the property itself.</b> Each tool's "
             f"scorer re-runs that tool's own artifact from source and checks it passes — Creusot: "
             f"<code>cargo creusot</code> reports every goal <i>Proved</i>; Kani: <code>cargo kani</code> reports "
             f"<i>VERIFICATION SUCCESSFUL</i> with the anti-vacuity <code>__mutant</code> twin going RED.</p>"
             f"<p style='margin:0 0 6px'>The two tools are complementary, so their individual counts differ and "
             f"neither alone covers everything. Where one tool cannot even <i>state</i> a property (e.g. Creusot's "
             f"logic model treats string contents as opaque), the other tool proves it — that is a sound "
             f"<b>hand-off</b>, not a gap. The per-method tables further down group properties by the method they "
             f"constrain; there <b>B</b> is simply the number of properties in that group (the "
             f"“bundle size”), and <b>proved / B</b> is how many of them that one tool proves by itself.</p></div>")
    P.append("<div class='scroll'><table><tr><th class='c'>symbol</th><th>meaning</th></tr>"
             "<tr><td class='c ok sym'>✓</td><td>proved by this tool, on the real Rust types</td></tr>"
             "<tr><td class='c star sym'>★</td><td>proved by this tool against a sound abstraction it needs "
             "(Creusot models the map/list as a logic-level FMap/Seq ghost-mirror, or a trusted boundary) — still "
             "this tool's own proof, just not over the concrete container</td></tr>"
             "<tr><td class='c deleg sym'>⤴</td><td>delegated — this tool did not prove it; the other tool did, "
             "soundly, and we point at that proof</td></tr>"
             "<tr><td class='c tb sym'>⊘</td><td>this tool cannot even state the property (a structural wall). When "
             "the other tool proves it, this is a sound hand-off, not an open gap; it only counts as open if "
             "<i>neither</i> tool can discharge it</td></tr>"
             "<tr><td class='c pend sym'>·</td><td>pending — not yet scored</td></tr></table></div>")

    # ---- Section: per-method scorecard, one table per tool (AS BEFORE) ----
    sec = 1
    for t in tools:
        mm = per_tool_methods[t]
        P.append(f"<h2>{sec} · {TOOL_LABEL.get(t,t)} — per-method scoring</h2>")
        sec += 1
        P.append(f"<p class='sub'>{agg[t]['proved']} of {M} properties proved by {TOOL_LABEL.get(t,t)} itself"
                 + (f"; {agg[t]['deleg']} delegated" if agg[t]['deleg'] else "")
                 + (f"; {agg[t]['tb']} carried by the other tool" if agg[t]['tb'] else "")
                 + (f"; {agg[t]['true_partial']} left open" if agg[t]['true_partial'] else "")
                 + ". <b>B</b> = bundle size (how many properties the method has); "
                 "<b>proved / B</b> = how many of them this tool proves by itself.</p>")
        P.append("<div class='scroll'><table><tr><th class='c'>#</th><th>method</th>"
                 "<th class='c'>proved / B</th><th class='c'>rating</th><th>delegated / handed off / open</th></tr>")
        for i, (m, ids) in enumerate(methods.items(), 1):
            r = mm[m]
            symch, symcls = frac_symbol(r)
            left = ""
            if r["left"]:
                left = "; ".join(f"<code>{esc(pid)}</code> {esc(disp)}" for pid, disp in r["left"])
            else:
                left = "<span class='foot'>—</span>"
            P.append(f"<tr><td class='c foot'>{i}</td><td><code>{esc(m)}</code></td>"
                     f"<td class='c'><span class='{symcls} sym'>{symch}</span> "
                     f"<span class='frac'>{r['nat']} / {r['B']}</span></td>"
                     f"<td class='c'><span class='rate {RATE_CLASS.get(r['rating'],'r-pend')}"
                     f"{' big-pill' if r['rating']=='partially proved' else ''}'>{esc(r['rating'])}</span></td>"
                     f"<td class='foot'>{left}</td></tr>")
        P.append("</table></div>")

    # ---- Section: the partial ones, drilled to properties (proved < B) ----
    P.append(f"<h2>{sec} · Where each tool leans on the other — the delegation detail</h2>")
    sec += 1
    P.append("<p class='sub'>These methods are fully <b>covered</b>, but not every bundle property is proved by "
             "this tool itself — the listed properties are soundly delegated to the other tool (⤴) or, if any, "
             "still open (⊘ / ·). This is the detail behind the covered count above.</p>")
    any_partial = False
    for t in tools:
        mm = per_tool_methods[t]
        partial_methods = [(m, mm[m]) for m in methods if mm[m]["left"]]
        if not partial_methods:
            continue
        any_partial = True
        P.append(f"<h3>{TOOL_LABEL.get(t,t)}</h3>")
        P.append("<div class='scroll'><table class='lg'><tr><th>method</th><th class='c'>proved / B</th>"
                 "<th>property</th><th>statement</th><th>disposition</th></tr>")
        for m, r in partial_methods:
            for j, (pid, disp) in enumerate(r["left"]):
                stmt = esc((props_by_id[pid].get("statement") or "").strip())
                mcell = (f"<td rowspan='{len(r['left'])}'><code>{esc(m)}</code></td>"
                         f"<td rowspan='{len(r['left'])}' class='c frac'>{r['nat']}/{r['B']}</td>") if j == 0 else ""
                dcls = "deleg" if disp.startswith("⤴") else ("tb" if disp.startswith("⊘") else "pend")
                P.append(f"<tr>{mcell}<td class='mono'>{esc(pid)}</td><td>{stmt}</td>"
                         f"<td class='{dcls}'>{esc(disp)}</td></tr>")
        P.append("</table></div>")
    if not any_partial:
        P.append("<p class='sub'>None — every method is proved outright by both tools.</p>")

    # ---- Section: per-property table ----
    P.append(f"<h2>{sec} · Per-property scoring — every verifiable obligation</h2>")
    sec += 1
    P.append("<div class='scroll'><table class='lg'><tr><th>property</th><th>method(s)</th><th>what it requires</th>"
             + "".join(f"<th class='c'>{TOOL_LABEL.get(t,t)}</th>" for t in tools)
             + "<th>note</th></tr>")
    def cellp(block):
        s = psym(block)
        cls = {"✓": "ok", "★": "star", "⤴": "deleg", "⊘": "tb", "·": "pend"}.get(s, "")
        return f"<td class='c sym {cls}'>{s}</td>"
    for p in props:
        note = ""
        for t in tools:
            b = p.get(t) or {}
            if b.get("status") in ("delegated", "tool-boundary") and b.get("note"):
                note = esc(b.get("note"))
                break
        ms = ", ".join(p.get("methods") or [])
        P.append(f"<tr><td class='mono'>{esc(p['id'])}</td><td class='foot'>{esc(ms)}</td>"
                 f"<td>{esc((p.get('statement') or '').strip())}</td>"
                 + "".join(cellp(p.get(t) or {}) for t in tools)
                 + f"<td class='foot'>{note}</td></tr>")
    P.append("</table></div>")

    # ---- Delegations ----
    P.append(f"<h2>{sec} · Where the tools cover for each other (delegations)</h2>")
    sec += 1
    P.append("<div class='scroll'><table><tr><th>property</th><th>delegating tool → owner</th>"
             "<th>why (reproduced)</th><th>proved by</th></tr>")
    any_del = False
    for p in props:
        for t in tools:
            b = p.get(t) or {}
            if b.get("status") == "delegated":
                any_del = True
                other = [x for x in tools if x != t]
                prover = ", ".join(TOOL_LABEL.get(x, x) for x in other
                                   if (p.get(x) or {}).get("status") == "proved") or "—"
                ev = b.get("evidence") or {}
                why = (ev.get("delegate_reason") or ev.get("bounded_shallow") or b.get("note") or "") if isinstance(ev, dict) else (b.get("note") or "")
                P.append(f"<tr><td class='mono'>{esc(p['id'])}</td>"
                         f"<td>{TOOL_LABEL.get(t,t)} → {esc(b.get('delegate_to') or 'other tool')}</td>"
                         f"<td class='foot'>{esc(why)}</td><td class='c ok'>{esc(prover)}</td></tr>")
    if not any_del:
        P.append("<tr><td colspan='4' class='foot'>No delegations.</td></tr>")
    P.append("</table></div>")

    # ---- DEFECTS FOUND (refuted obligations) ----
    # The headline result when it happens: verification did its job and the CODE failed. Given its
    # own prominent section, above the tool-boundary discussion, with the spec place and the code
    # lines a reader needs in order to act — the point of the exercise is a fix, not a score.
    refs = [p for p in props if any((p.get(t) or {}).get("status") == "refuted" for t in tools)]
    if refs:
        P.append(f"<h2>{sec} · ‼ Defects found — obligations the code violates</h2>")
        sec += 1
        P.append("<div class='note' style='border-left:4px solid #b00;padding-left:10px'>"
                 "Each row is a property that was <b>machine-checked to be FALSE</b>: the implementation "
                 "breaks it. These are findings, not gaps in the verification — they are the return on "
                 "the verification effort, and each needs a decision in the spec, the code, or both.</div>")
        P.append("<div class='scroll'><table><tr><th>property</th><th>by</th><th>spec</th>"
                 "<th>code location</th><th>what is violated</th><th>witness</th></tr>")
        for p in refs:
            src = p.get("source") or {}
            spec_loc = ", ".join(str(x) for x in (src.get("spec") or p.get("traces") or [])) or "—"
            code_loc = ", ".join(str(x) for x in (src.get("code") or [])) or "—"
            for t in tools:
                b = p.get(t) or {}
                if b.get("status") != "refuted":
                    continue
                ev = b.get("evidence") or {}
                wit = ev.get("refutation", "") if isinstance(ev, dict) else ""
                P.append(
                    f"<tr><td class='mono'>{esc(p['id'])}</td><td>{TOOL_LABEL.get(t,t)}</td>"
                    f"<td class='mono'>{esc(spec_loc)}</td><td class='mono'>{esc(code_loc)}</td>"
                    f"<td>{esc(str(p.get('statement','')).strip())}</td>"
                    f"<td class='mono'>{esc(str(wit))}</td></tr>")
        P.append("</table></div>")

    # ---- Tool-boundary ----
    tbs = [p for p in props if any((p.get(t) or {}).get("status") == "tool-boundary" for t in tools)]
    P.append(f"<h2>{sec} · Tool-boundary rows and their causes</h2>")
    sec += 1
    if tbs:
        P.append("<div class='scroll'><table><tr><th>property</th><th>tool</th><th>miss_class</th>"
                 "<th>reproduced signature</th></tr>")
        for p in tbs:
            for t in tools:
                b = p.get(t) or {}
                if b.get("status") == "tool-boundary":
                    ev = b.get("evidence") or {}
                    sig = ev.get("signature") if isinstance(ev, dict) else ""
                    P.append(f"<tr><td class='mono'>{esc(p['id'])}</td><td>{TOOL_LABEL.get(t,t)}</td>"
                             f"<td>{esc(b.get('miss_class'))}</td><td class='foot mono'>{esc(sig)}</td></tr>")
        P.append("</table></div>")
    else:
        P.append("<p class='sub'>None — every obligation is proved by ≥1 tool (some via a documented delegation).</p>")

    # ---- Measurement ----
    P.append(f"<h2>{sec} · Measurement</h2>")
    sec += 1
    P.append("<div class='scroll'><table><tr><th>tool</th><th>Σ wall-clock</th><th>peak RSS</th><th>discharged</th></tr>")
    for t in tools:
        A = agg[t]
        disc = f"{A['artifacts']} {'proof modules (.coma)' if t=='creusot' else 'harnesses'}, {A['proved']} properties proved"
        P.append(f"<tr><td>{TOOL_LABEL.get(t,t)}</td><td>{A['wall']}s</td><td>{A['rss']}MB</td><td>{disc}</td></tr>")
    P.append("</table></div>")
    # Say EXACTLY what this number is. The earlier wording ("the sum over per-property scorer runs")
    # read as total machine cost and was not: it omits the anti-vacuity and lever invocations, and it
    # bundles toolchain build time into each figure. Measured on remote-lookup, the honest total was
    # 2874s against a 1031s reported sum — a 2.8x understatement in a page meant to be citable.
    P.append(
        "<p class='foot'><b>What these times include.</b> Each figure is the sum of the "
        "<i>first</i> proof attempt per property, as measured by the scorer around one whole "
        "toolchain invocation (<code>/usr/bin/time -v cargo kani --harness &lt;h&gt;</code> or "
        "<code>cargo creusot &lt;module&gt;</code>). So it is <b>build + proof</b>, not solver time: "
        "a sub-second proof still shows seconds because compilation is counted with it, and Kani "
        "re-invokes the toolchain per harness while Creusot's runs are largely incremental — which is "
        "most of why Kani's per-property figures sit well above Creusot's. A figure at or near the cap "
        "(<code>--cap-seconds</code>, escalating once to <code>--cap-max</code>) is a <b>timeout, not "
        "solving</b>.<br>"
        "<b>What they exclude.</b> The scorer also runs an anti-vacuity <code>__mutant</code> twin for "
        "each proved property and the full lever battery for each tool-boundary; those invocations are "
        "<b>not</b> counted here, so one property can cost 2–5 runs while only the first is timed. "
        "Total machine cost per tool is the stage wall-clock in "
        "<code>verif/.run/&lt;stage&gt;.done</code>, which is larger than the sum above. Peak RSS is "
        "the largest single timed run, not a total. Agent time to author the proofs is not measured "
        "anywhere on this page.</p>")

    # ---- Anti-vacuity / provenance ----
    P.append(f"<h2>{sec} · Anti-vacuity & provenance</h2>")
    P.append("<div class='callout'><b>Anti-vacuity.</b> Status is written only by the reproduction scorers, which "
             "re-run each artifact from <code>touch</code>ed sources (defeating a stale proof cache). Where a "
             "<code>__mutant</code> twin exists it must FAIL; a mutant that also passes forces UNRESOLVED and fails "
             "the gate — this is how a bounded-shallow harness that would pass vacuously is caught and routed to a "
             "sound proof by the other tool. This component's delegations, if any, are listed in the delegations "
             "section above.</div>")
    P.append(f"<p class='foot'>Provenance — component <code>{esc(comp)}</code>, interface <code>{esc(interface)}</code>, "
             f"pin <code>{esc(pin)}</code>. Source of truth <code>unified_properties.yaml</code>. "
             f"Rendered by <code>render_scoring.py</code>. No <code>.md</code> backing file.</p>")

    # Which GATE produced these statuses. Written by the scorers into `run:`; shown here so the
    # page is self-describing — a reader can tell exactly which code and toolchain scored it,
    # and whether a re-score today would be running the same gate.
    run = d.get("run") or {}

    # UNVERIFIED COLUMN WARNING. A tool column can be fully populated with scorer-owned statuses
    # and still be unreproducible — the proof artifacts may no longer exist. `--require-complete`
    # cannot catch that: it checks that a status is PRESENT, not that it could be re-derived. The
    # signal is a tool with scored cells but no `run.<tool>` provenance block, meaning no recorded
    # run of this gate ever produced it. Observed for real: a component whose Creusot crate was
    # deleted still rendered as a complete deliverable, its 16 "proved" cells unsupported by any
    # artifact. Say so on the page, loudly, so a reader cannot cite it unaware.
    unverified = []
    for tool in tools:
        scored = sum(1 for p in props if (p.get(tool) or {}).get("_scored_by"))
        if scored and not isinstance(run.get(tool), dict):
            unverified.append((tool, scored))
    if unverified:
        warn = "; ".join(f"<b>{esc(t)}</b>: {n} scored cells" for t, n in unverified)
        P.append(
            "<p class='foot' style='border:2px solid #b00;padding:8px'>"
            "⚠ <b>UNVERIFIED COLUMN — do not cite without re-verifying.</b> "
            f"{warn} carry scorer-owned statuses but NO run provenance, so no recorded run of the "
            "gate produced them and they could not be reproduced now. This usually means the proof "
            "artifacts no longer exist. Completeness checks cannot detect this: they confirm a "
            "status is present, not that it can be re-derived.</p>")

    if isinstance(run, dict) and run:
        bits = []
        gc, gb = run.get("gate_commit"), run.get("gate_branch")
        if gc:
            bits.append(f"gate <code>{esc(str(gc))}</code>" + (f" on <code>{esc(str(gb))}</code>" if gb else ""))
        if run.get("gate_dirty"):
            bits.append("<b>gate had uncommitted edits when this ran</b> — the commit above does "
                        "not fully describe the code that scored it")
        for tool in ("creusot", "kani"):
            blk = run.get(tool)
            if not isinstance(blk, dict):
                continue
            seg = []
            for key, label in (("creusot", "Creusot"), ("kani_version", ""), ("why3", ""),
                               ("provers", "provers"), ("finished", "finished")):
                v = blk.get(key)
                if v is None:
                    continue
                v = ", ".join(str(x) for x in v) if isinstance(v, list) else str(v)
                seg.append(f"{label} {esc(v)}".strip())
            cmd = blk.get("command")
            if cmd:
                seg.append(f"<code>{esc(str(cmd))}</code>")
            if seg:
                bits.append(f"<b>{tool}</b>: " + " · ".join(seg))
        if bits:
            P.append("<p class='foot'>Run provenance — " + "<br>".join(bits) + "</p>")

    P.append("</div>")

    # --require-complete: a verifiable property is "complete" for a rendered tool iff its tool
    # block carries a scorer-owned status. Any gap means this is not the shippable page: write it
    # under the INCOMPLETE name (still inspectable) and exit non-zero, so the pipeline can never
    # publish a partial run under the deliverable name.
    missing = [(p["id"], t) for p in props for t in tools if "status" not in (p.get(t) or {})]
    if a.require_complete and missing:
        if out.endswith(".html"):
            out = out[:-5] + ".INCOMPLETE.html"
        else:
            out = out + ".INCOMPLETE"

    with open(out, "w") as f:
        f.write("\n".join(P))
    print(f"render_scoring: wrote {out}")
    if a.require_complete and missing:
        print(f"render_scoring: REQUIRE-COMPLETE FAILED — {len(missing)} unscored (property,tool) "
              f"cell(s), e.g. {missing[:6]}. Wrote the INCOMPLETE page above, NOT the deliverable name.")
        sys.exit(2)
    for t in tools:
        A = agg[t]
        print(f"  {TOOL_LABEL.get(t,t):8s}: {A['fully']}/{N} methods proved outright "
              f"({A['partial']} partial, {A['deleg_methods']} delegated, {A['true_partial']} open; "
              f"{A['covered']}/{N} covered union) | "
              f"props {A['proved']} proved / {A['deleg']} delegated / {A['tb']} tb / "
              f"{A['pend']} pending" + (f" / ‼ {A['refuted']} REFUTED (defects found)" if A.get('refuted') else ""))


if __name__ == "__main__":
    main()

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
import argparse, os, re, sys, html, collections, datetime
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


def apply_polarity(d):
    """Read HAZARD-shaped records the right way round. Display only: the bundle is not changed.

    An obligation normally states what the code SHOULD do, so `proved` = the requirement holds and
    `refuted` = a defect. Measured on memory-tier (2026-10-03): 22 of its 161 records instead state a
    DEFECT ("a new entry's bytes can still hold a previous occupant's data") — the sweep's code reader
    wrote its surprises as records. For those, `proved` CONFIRMS the defect and `refuted` shows the
    code is correct, so rendered as-is the page shows every one of them with the wrong sign.

    The classification lives in an optional top-level `polarity:` map ({id: {polarity, why}}). Only
    the rendering flips; the scorer-written statuses are untouched, and every flipped cell says so.
    Returns {id: polarity} for the records that are not plain requirements, for the legend.
    """
    pol = d.get("polarity") or {}
    seen = {}
    for p in d.get("properties", []):
        e = pol.get(p.get("id"))
        if not isinstance(e, dict):
            continue
        kind = str(e.get("polarity", "")).upper()
        if kind in ("", "REQUIREMENT"):
            continue
        seen[p["id"]] = kind
        if kind != "HAZARD":
            continue
        for t in TOOLS_DEFAULT:
            b = p.get(t)
            if not isinstance(b, dict):
                continue
            st = b.get("status")
            if st == "proved":
                b["status"], b["symbol"] = "refuted", "\u203c"
                b["note"] = ("DEFECT CONFIRMED. This record is stated as a hazard, and the hazard was "
                             "PROVED to occur. " + str(b.get("note") or ""))
            elif st == "refuted":
                b["status"], b["symbol"] = "proved", "\u2713"
                b["note"] = ("CODE IS CORRECT. This record is stated as a hazard, and the hazard was "
                             "proved NOT to occur. " + str(b.get("note") or ""))
    return seen


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
    if st == "open":
        # An agreed property that the tools did not prove. Not a finding on the page: it is open
        # work, or a bug candidate for the repair triage; a bug is shown once it is FIXED.
        return "○"
    if st == "discordance":
        # Spec and code DISAGREE and verification confirmed which side the code follows. Side
        # information for whoever owns spec<->code synchronisation; not a defect, not a gap.
        return "≠"
    if st == "wording":
        # False only as worded (e.g. "always smaller" where the code keeps <=); the code does
        # what was meant. Not a defect.
        return "≈"
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
    polarity_seen = {}   # hazard flips retired 2026-10-04: hazard records are code-only, out of scope
    # `triage:` {id: {verdict: not-a-defect, why}} — orchestrator-written. A refutation is a mechanical
    # fact (the obligation AS WORDED is false); whether that is a CODE defect is a judgement. memory-tier
    # has 4 that are not: three refuted only on wording (e.g. "always smaller" where the code keeps <=)
    # and one on a u64 counter wrap that cannot be reached. Shown, labelled, and kept out of the count.
    triage, discord = {}, {}   # triage labels retired 2026-10-04 (see SCOPE): not shown on the page
    # A DEFECT is pronounced only after verification, and only for an obligation the code violates
    # whatever the spec intends: one spec and code agree on, or a safety obligation (panic, overflow,
    # out-of-bounds). A spec<->code disagreement that verification merely CONFIRMS is a discordance:
    # side information for the spec-sync owners, shown separately, and it does not stop a method
    # counting as verified (Cornel, 2026-10-03). Display only; the scorer's statuses are untouched.
    for p_ in d.get("properties", []):
        v = p_.get("id")
        new = "discordance" if v in discord else ("wording" if v in triage else None)
        if not new:
            continue
        for t_ in TOOLS_DEFAULT:
            b_ = p_.get(t_)
            if isinstance(b_, dict) and b_.get("status") == "refuted":
                b_["status"] = new
    comp = d.get("component", "component")
    interface = d.get("interface", "")
    pin = d.get("pin", "")
    counts = d.get("counts", {})
    tools = d.get("tools") or (counts.get("tools") if isinstance(counts.get("tools"), list) else None) or TOOLS_DEFAULT
    out = a.out or os.path.join(verif, f"{comp}_scoring.html")

    props = [p for p in d["properties"] if p.get("verifiable")]
    nv = [p for p in d["properties"] if not p.get("verifiable")]
    # SCOPE = what the specification and the code AGREE on (origin spec+code), as on the 2026-09-22
    # pages. Measured 2026-10-04: from 2026-09-28 the inventory fed the provers the UNION (spec-only,
    # code-only, divergent), and 43 of the 46 "refutations" across four components came from records
    # that only restated a disagreement the extraction had already found -- verification was not
    # finding them. Restricted to agreed properties: dpm 0 refuted (4/4 methods), session-lists 0
    # (9/9), memory-tier 2 (10/17). The other records stay in the YAML as extraction data; they are
    # not obligations and are not shown. An agreed property that does not prove is shown as NOT
    # VERIFIED; a bug is shown only once it is fixed (`bugs:` with status fixed).
    # Refined the same night after measuring every published page: the pages of 2026-09-22..27 DID
    # include code-only properties (the code's own documented intent) and were clean, e.g.
    # block-device-filesys 28/28 with 13 code-only. What produced the flood is narrower: DIVERGENT
    # records (spec and code disagree), SPEC-ONLY records (a requirement the code does not implement)
    # and HAZARD-worded records. Those are spec<->code synchronisation, not verification. Origin
    # labels vary by run ("spec+code"/"both"/"paired", "code-only"/"code").
    IN_SCOPE = {"spec+code", "both", "paired", "code-only", "code"}
    _pol = d.get("polarity") or {}
    all_method_names = list(dict.fromkeys(m for p in props for m in (p.get("methods") or [])))
    outside = []
    if any(p.get("origin") for p in props):
        _excl = set(d["level2_excluded"]) if isinstance(d.get("level2_excluded"), list) else None
        def _in(p):
            if str((_pol.get(p["id"]) or {}).get("polarity", "")).upper() == "HAZARD":
                return False
            if _excl is not None:                  # LEVEL 1 decided (level1.py / discordances.yaml)
                return p["id"] not in _excl
            return p.get("origin") in IN_SCOPE     # older bundle: the origin rule
        outside = [p for p in props if not _in(p)]
        props = [p for p in props if _in(p)]
        for p_ in props:
            for t_ in TOOLS_DEFAULT:
                b_ = p_.get(t_)
                if isinstance(b_, dict) and b_.get("status") == "refuted":
                    b_["status"] = "open"
    props_by_id = {p["id"]: p for p in props}

    # public methods (ordered) + bundles (property ids attached to each)
    methods = collections.OrderedDict()
    extra_methods = []
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
        N = counts.get("methods") or len(all_method_names)   # full method set, not the in-scope one
        # The method headline is "X of N interface methods verified", so it must count ONLY the
        # interface's methods. Extraction also attaches properties to other entry points (Drop,
        # Default, inherent helpers): measured on memory-tier, 22 names against a 17-method trait,
        # which produced "4 / 17 verified + 17 more" = 21 of 17. `interface_methods` (orchestrator-
        # written, read from the trait itself) fixes the denominator; the other entry points keep
        # their properties in every property-level count and are named under the headline.
        im = d.get("interface_methods")
        if im:
            extra_methods = [m for m in methods if m not in im]
            methods = collections.OrderedDict((m, methods[m]) for m in im if m in methods)
            N = len(im)
        else:
            extra_methods = []

    # ---- per (tool, method) native fraction + disposition of the remainder ----
    # returns dict: nat, B, rating, leftovers=[(pid, disp)]
    def method_row(tool, ids):
        nat = deleg = tb = pend = handed = refd = acc = 0
        left = []          # non-native bundle properties (the delegation/gap detail)
        blocking = []      # leftovers that keep the method from `verified`
        for pid in ids:
            b = props_by_id[pid].get(tool) or {}
            s = psym(b)
            if s in ("✓", "★"):
                nat += 1
            elif s == "≠":
                acc += 1
                left.append((pid, "≠ spec and code disagree; verification confirmed what the code does "
                                  "(side information, not a defect)"))
                continue
            elif s == "≈":
                acc += 1
                left.append((pid, "≈ false only as worded; the code does what was meant (not a defect)"))
                continue
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
            elif s == "○":
                pend += 1
                left.append((pid, "○ not verified"))
            else:
                pend += 1
                left.append((pid, "· pending"))
            if s not in ("✓", "★", "⤴") and not (s == "⊘" and any(
                    psym(props_by_id[pid].get(ot) or {}) in ("✓", "★") for ot in tools if ot != tool)):
                blocking.append(pid)
        B = len(ids)
        covered = nat + deleg + handed + acc  # proved here, delegated, proved by the other tool, or ≠/≈
        gap = B - covered                   # genuinely open: pending, or a wall no tool clears
        # rating is FAIR / per-tool: a method is `proved` for a tool ONLY when that tool
        # proves EVERY bundle property itself (proved == B). If it proves some and leaves
        # the rest to the other tool it is `partially proved`; if it proves none, the whole
        # bundle is `delegated` (or `needs other tool` on a structural wall).
        # GLOBAL INVARIANTS, as on the 2026-09-22 pages: one or two shared properties that fail can
        # make EVERY method look unverified, which says nothing about the methods themselves. A method
        # whose only leftovers are shared global properties is `proved except shared invariants`,
        # and the headline names those invariants once instead of once per method.
        glob_block = [x for x in blocking if props_by_id[x].get("global")]
        own_block = [x for x in blocking if not props_by_id[x].get("global")]
        if nat + acc == B:
            rating = "proved"
        elif blocking and not own_block and nat + acc + deleg + handed + len(glob_block) == B:
            rating = "proved except shared invariants"
        elif nat == 0:
            rating = ("delegated" if (deleg and not tb and not pend)
                      else ("needs other tool" if tb else "pending"))
        else:
            rating = "partially proved"
        return dict(nat=nat, B=B, deleg=deleg, tb=tb, pend=pend, refuted=refd, acc=acc,
                    covered=covered, gap=gap, rating=rating, left=left,
                    glob_block=glob_block, own_block=own_block)

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

    # A tool whose column is explained by a `tool_notes` entry (withheld, sample-only, unfinished) is
    # pending by DECISION, and the note already says so in the one-line notice at the top. Raising the
    # generic INCOMPLETE box for it as well put a second, vaguer warning above every count.
    noted = {str(n.get("tool", "")).lower() for n in (d.get("tool_notes") or [])}
    incomplete = any(agg[t]["true_partial"] and agg[t]["pend"] for t in tools if t not in noted)
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

    RATE_CLASS = {"proved": "r-proved", "proved except shared invariants": "r-partial", "partially proved": "r-partial",
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

    # CORRECTIONS AND NOTES - one short line at the top, full text at the END of the page.
    # Two kinds exist: `known_corrections` (one PROPERTY's published verdict is superseded; a status
    # may only be written by a scorer, so the bundle cannot simply be edited) and `tool_notes` (a whole
    # tool COLUMN is withheld or must not be read). Both must be impossible to miss, because the page
    # would otherwise show a superseded or meaningless number in silence. They used to render as full
    # red-boxed paragraphs ABOVE every count; Cornel (2026-10-03): this page is meant to be simple to
    # digest for colleagues who know nothing about it, so the top carries ONE line saying how many
    # there are and what they touch, linking to a section at the end that holds the full text.
    kcs = d.get("known_corrections") or []
    tns = d.get("tool_notes") or []
    notes_html = []
    for kc in kcs:
        notes_html.append(
            "<p class='foot' style='border-left:4px solid #b00;padding:4px 10px'>"
            f"<b>Correction — <code>{esc(str(kc.get('property','')))}</code></b> is shown as "
            f"<b>{esc(str(kc.get('this_bundle_says','')))}</b> but should read "
            f"<b>{esc(str(kc.get('should_read','')))}</b>. {esc(str(kc.get('why','')))} "
            f"<b>{esc(str(kc.get('not_a_defect','')))}</b> "
            f"Corrected counts: <b>{esc(str(kc.get('correct_counts','')))}</b>. "
            f"Reproduce: <code>{esc(str(kc.get('reproduce','')))}</code></p>")
    for tn in tns:
        notes_html.append(
            "<p class='foot' style='border-left:4px solid #b00;padding:4px 10px'>"
            f"<b>{esc(str(tn.get('tool','')).capitalize())} column — "
            f"{esc(str(tn.get('headline','')))}</b> "
            f"{esc(str(tn.get('detail','')))}"
            + (f" <b>Measured:</b> {esc(str(tn.get('measured','')))}." if tn.get('measured') else "")
            + "</p>")
    nhaz = sum(1 for v in polarity_seen.values() if v == "HAZARD")
    nunc = sum(1 for v in polarity_seen.values() if v != "HAZARD")
    if nhaz or nunc:
        notes_html.append(
            "<p class='foot' style='border-left:4px solid #b00;padding:4px 10px'>"
            f"<b>{nhaz} records are worded as hazards, not requirements.</b> Most records say what the "
            "code SHOULD do, so a proof means the requirement holds. These instead describe something "
            "that might go WRONG, so a proof means the problem is real. This page shows them the right "
            "way round: a proved hazard is listed as a defect (\u203c), and a hazard proved not to occur "
            "is shown as correct (\u2713). Each such cell says so in its note. The verification results "
            "themselves are unchanged."
            + (f" {nunc} further record(s) could not be classified either way and are shown as written."
               if nunc else "") + "</p>")
    if triage:
        notes_html.append(
            "<p class='foot' style='border-left:4px solid #b00;padding:4px 10px'>"
            f"<b>{len(triage)} refuted records are not code defects.</b> Each is false exactly as worded, "
            "but the code does what was meant: " + "; ".join(
                f"<code>{esc(k)}</code>: {esc(str(v.get('why','')))}" for k, v in sorted(triage.items()))
            + ". They stay in the defects table, labelled, and are not counted as defects.</p>")
    if notes_html:
        parts = []
        if nhaz:
            parts.append(f"{nhaz} records are worded as hazards and shown the right way round")
        if triage:
            parts.append(f"{len(triage)} refuted records are not code defects")
        if kcs:
            parts.append(f"{len(kcs)} correction{'s' if len(kcs) != 1 else ''} (" +
                         ", ".join(esc(str(k.get('property',''))) for k in kcs) + ")")
        if tns:
            parts.append(" and ".join(f"the {esc(str(t.get('tool','')).capitalize())} column is "
                                      f"{esc(re.split(r'[,.;:]', str(t.get('headline','')))[0].strip())}"
                                      for t in tns))
        P.append("<p class='foot' style='border-left:4px solid #b00;padding:2px 10px'>⚠ "
                 + "; ".join(parts) + ". <a href='#notes'>Details at the end of the page.</a></p>")

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
    # Union view of what blocks each method: a property blocks only if NO tool settles it.
    def settled(pid):
        return any(psym(props_by_id[pid].get(t) or {}) in ("✓", "★", "⤴", "≠", "≈") for t in tools)
    m_glob_only, m_own, glob_hits = [], [], collections.Counter()
    for m, ids in methods.items():
        blk = [x for x in ids if not settled(x)]
        if not blk:
            continue
        if all(props_by_id[x].get("global") for x in blk):
            m_glob_only.append(m)
            glob_hits.update(blk)
        else:
            m_own.append(m)
    def why_open(pid):
        st = {psym(props_by_id[pid].get(t) or {}) for t in tools}
        return "a defect" if "‼" in st else ("a tool boundary" if "⊘" in st else "not yet proved")

    P.append("<div class='cards'>")
    mhead_cls = "ok" if m_open == 0 else "warn"
    mtail = (f"<b>{m_open}</b> still open" if m_open
             else "<b>0</b> open — every method fully covered")
    if m_glob_only:
        gl = ", ".join(f"<code>{esc(g)}</code> ({why_open(g)}; touches {n} method{'s' if n != 1 else ''})"
                       for g, n in glob_hits.most_common())
        mtail = (f"<b>{len(m_glob_only)}</b> more are verified on their own properties and held back only "
                 f"by {len(glob_hits)} shared global propert{'ies' if len(glob_hits) != 1 else 'y'}: {gl}"
                 + (f"; <b>{len(m_own)}</b> have an open property of their own" if m_own else "")
                 + (f". {mtail[0].upper()}{mtail[1:]}" if False else ""))
    _dpath = os.path.join(verif, "discordances.yaml")
    if os.path.exists(_dpath):
        _dn = (yaml.safe_load(open(_dpath)) or {}).get("counts", {}).get("discordances", 0)
        if _dn:
            mtail += (f". <b>Level 1</b>: reconciling the specification with the code found <b>{_dn}</b> "
                      f"spec&harr;code discordance{'s' if _dn != 1 else ''}; the properties built from them "
                      f"are not formally verified here (<a href='{esc(comp)}_discordances.html'>see the list</a>)")
    im_all = d.get("interface_methods") or all_method_names
    no_scope = [m for m in im_all if m not in methods and m in all_method_names]
    if no_scope:
        mtail += (f". {len(no_scope)} method{'s have' if len(no_scope) != 1 else ' has'} no property the "
                  f"specification and the code agree on ("
                  + ", ".join(f"<code>{esc(x)}</code>" for x in no_scope) + ")")
    if extra_methods:
        mtail += (f". Properties are also attached to {len(extra_methods)} entry point"
                  f"{'s' if len(extra_methods) != 1 else ''} outside the interface ("
                  + ", ".join(f"<code>{esc(x)}</code>" for x in extra_methods)
                  + "); they count in every property figure but not in this method total")
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
    n_bugs = len({(d.get("triage") or {}).get(p_["id"], {}).get("bug") or ("?" + p_["id"])
                  for p_ in props if any((p_.get(t) or {}).get("status") == "refuted" for t in tools)})
    n_triaged = sum(1 for p in props if p.get("id") in triage
                    and any((p.get(t) or {}).get("status") == "refuted" for t in tools))
    P.append("<div class='cards'>")
    head_cls = "ok" if c_open == 0 else "warn"   # refuted is settled, so it does not warn here
    combfoot = []
    if c_deleg:
        combfoot.append(f"<b>{c_deleg}</b> soundly delegated to a named external referent")
    if c_open:
        # Say WHAT is open. Counting is unchanged; only the label is. A refuted property was being
        # reported as a bare "open", which reads as unfinished verification when in fact the
        # verification finished and proved the implementation wrong.
        note = (f" &mdash; <b>‼ {c_refuted} of these show {n_bugs} bug{'s' if n_bugs != 1 else ''} "
                f"(see Bugs found)</b>"
                + (f"; <b>{n_triaged}</b> of those are false only as worded, not code defects"
                   if n_triaged else "")) if c_refuted else ""
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
        tn_ = next((n for n in (d.get("tool_notes") or []) if str(n.get("tool", "")).lower() == t), None)
        if tn_:
            P.append(f"<div class='kpi'><div class='big'>{esc(TOOL_LABEL.get(t,t))}</div>"
                     f"<div class='lbl'>{esc(str(tn_.get('headline','')))}</div>"
                     f"<div class='foot' style='margin-top:8px'>{A['proved']} properties proved in that "
                     f"run; see the note at the end of the page.</div></div>")
            continue
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
                + (f" ({n_triaged} of them are false only as worded, not code defects)" if n_triaged else "")
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
    # A tool whose column a tool_note explains (sample-only, withheld, unfinished) gets no per-method
    # sections: measured on memory-tier, its 157 unscored properties filled 570 of the 674 rows in the
    # detail section with "· pending", which read as "everything failed". The note says why instead.
    tools_m = [t for t in tools if t not in noted]
    for t in tools_m:
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
    # The old wording opened "These methods are fully covered", then contradicted itself with
    # "or, if any, still open". The table is selected by `mm[m]["left"]` — EVERY method with a
    # leftover — not by gap == 0, so the claim was false for any method with an unsettled property:
    # measured on eviction-policy-optimized, it listed all 9 methods while only 4 were covered.
    # State what the rows are instead of asserting a coverage level the selection does not check.
    P.append("<p class='sub'>Each row is a property this tool did <b>not</b> prove itself. That is not "
             "automatically a gap — read the disposition. <b>⊘ tool-boundary</b>: this tool "
             "<b>cannot prove it</b> — either the obligation is not expressible in its model, or the "
             "full escalation battery was exhausted without a verdict. The arrow that follows names the "
             "other tool, where that same obligation <i>is</i> proved, so the property stays covered. "
             "<b>⤴ delegated</b> is a different thing, not a tool limit: the obligation is sound but "
             "belongs to a named referent <i>outside</i> this pipeline (a concurrency model, the "
             "component framework), so neither tool here claims it. <b>○ not verified</b> — "
             "nothing settles it yet. Only that leaves the method short of full coverage.</p>")
    any_partial = False
    for t in tools_m:
        mm = per_tool_methods[t]
        # Shared global properties are listed ONCE, below, not under every method they touch:
        # on memory-tier one of them was repeated under 34 methods.
        glob_left = collections.OrderedDict()
        for m_ in methods:
            for pid_, disp_ in mm[m_]["left"]:
                if props_by_id[pid_].get("global"):
                    glob_left.setdefault(pid_, [disp_, 0])[1] += 1
        own_left = {m_: [(x, y) for x, y in mm[m_]["left"] if not props_by_id[x].get("global")] for m_ in methods}
        partial_methods = [(m, dict(mm[m], left=own_left[m])) for m in methods if own_left[m]]
        if not partial_methods and not glob_left:
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
        if glob_left:
            P.append(f"<p class='sub'><b>Shared global properties</b> this tool did not prove itself, each "
                     f"listed once with the number of methods it touches.</p>")
            P.append("<div class='scroll'><table class='lg'><tr><th>property</th><th class='c'>methods</th>"
                     "<th>statement</th><th>disposition</th></tr>")
            for pid_, (disp_, n_) in glob_left.items():
                dcls = "deleg" if disp_.startswith("⤴") else ("tb" if disp_.startswith("⊘") else "pend")
                P.append(f"<tr><td class='mono'>{esc(pid_)}</td><td class='c'>{n_}</td>"
                         f"<td>{esc((props_by_id[pid_].get('statement') or '').strip())}</td>"
                         f"<td class='{dcls}'>{esc(disp_)}</td></tr>")
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

    # ---- Delegations ---- (rendered only when there is one; an empty section is noise)
    _del_at, _del_sec = len(P), sec
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
    P.append("</table></div>")
    if not any_del:
        del P[_del_at:]
        sec = _del_sec

    # ---- DEFECTS FOUND (refuted obligations) ----
    # The headline result when it happens: verification did its job and the CODE failed. Given its
    # own prominent section, above the tool-boundary discussion, with the spec place and the code
    # lines a reader needs in order to act — the point of the exercise is a fix, not a score.
    refs = [p for p in props if any((p.get(t) or {}).get("status") == "refuted" for t in tools)]
    if refs:
        # BUGS, BY ROOT CAUSE (Cornel, 2026-10-03). A bug is a defect in the code; each refuted
        # property it explains is one MANIFESTATION of it (panic, out-of-bounds, wrong object, ...).
        # A team-mate reads "bug X, manifested as overflow here and as a wrong object there", not a
        # flat list of failed properties. A defect with no identified root cause is grouped last.
        bugs_meta = d.get("bugs") or {}
        tri_all = d.get("triage") or {}
        groups = collections.OrderedDict()
        for p in refs:
            bid = (tri_all.get(p["id"]) or {}).get("bug") or "_unknown"
            groups.setdefault(bid, []).append(p)
        order = [k for k in sorted(groups) if k != "_unknown"] + (["_unknown"] if "_unknown" in groups else [])
        P.append(f"<h2>{sec} · ‼ Bugs found — by root cause</h2>")
        sec += 1
        nb = sum(1 for k in order if k != "_unknown")
        P.append("<div class='note' style='border-left:4px solid #b00;padding-left:10px'>"
                 f"Verification found <b>{nb} bug{'s' if nb != 1 else ''}</b>, by root cause, showing up in "
                 f"<b>{len(refs)}</b> propert{'ies' if len(refs) != 1 else 'y'}. Each property row is one "
                 "place the bug shows, and says how: a crash, an out-of-bounds access, acting on the wrong "
                 "entry, and so on. Each was <b>machine-checked</b>: the property is false of the code.</div>")
        P.append("<div class='scroll'><table><tr><th>property</th><th>manifested as</th><th>by</th>"
                 "<th>what should hold</th><th>witness</th></tr>")
        for bid in order:
            bm = bugs_meta.get(bid) or {}
            if bid == "_unknown":
                hdr = "<b>Root cause not yet identified</b>"
            else:
                st_ = str(bm.get("status", "open"))
                hdr = (f"<b>{esc(bid)} — {esc(str(bm.get('title','')))}</b> "
                       f"<span class='foot'>[{esc(st_)}{(': ' + esc(str(bm.get('fix')))) if bm.get('fix') else ''}]</span>"
                       f"<br>{esc(str(bm.get('root_cause','')))}"
                       + (f"<br><span class='foot'>{esc(str(bm.get('location')))}</span>" if bm.get('location') else ""))
            P.append(f"<tr><td colspan='5' style='background:var(--partbg)'>{hdr}</td></tr>")
            for p in groups[bid]:
                man_ = (tri_all.get(p["id"]) or {}).get("manifested_as", "")
                for t in tools:
                    b_ = p.get(t) or {}
                    if b_.get("status") != "refuted":
                        continue
                    ev = b_.get("evidence") or {}
                    wit = (ev.get("refutation") or ", ".join(ev.get("modules") or [])) if isinstance(ev, dict) else ""
                    P.append(f"<tr><td class='mono'>{esc(p['id'])}</td><td>{esc(str(man_))}</td>"
                             f"<td>{TOOL_LABEL.get(t,t)}</td>"
                             f"<td>{esc(str(p.get('statement','')).strip())}</td>"
                             f"<td class='mono'>{esc(str(wit))}</td></tr>")
        P.append("</table></div>")

    fixed = {k: v for k, v in (d.get("bugs") or {}).items() if str(v.get("status")) == "fixed"}
    if fixed:
        P.append(f"<h2>{sec} · Bugs found and fixed</h2>")
        sec += 1
        P.append("<div class='scroll'><table><tr><th>bug</th><th>what was wrong</th><th>fix</th></tr>")
        for k, v in sorted(fixed.items()):
            P.append(f"<tr><td class='mono'>{esc(k)}</td><td><b>{esc(str(v.get('title','')))}</b><br>"
                     f"{esc(str(v.get('root_cause','')))}</td><td>{esc(str(v.get('fix','')))}</td></tr>")
        P.append("</table></div>")

    # ---- Tool-boundary ----
    tbs = [p for p in props if any((p.get(t) or {}).get("status") == "tool-boundary" for t in tools)]
    dis = [p_ for p_ in props if any((p_.get(t) or {}).get("status") == "discordance" for t in tools)]
    if dis:
        P.append(f"<h2>{sec} · ≠ Spec and code disagree — side information, not defects</h2>")
        sec += 1
        P.append("<div class='note'>Each row is a property on which the specification and the code "
                 "<b>disagree</b>, and verification confirmed what the code actually does. Keeping spec and "
                 "code in step is not this pipeline's job, so these are reported for whoever owns that, and "
                 "they do <b>not</b> stop a method counting as verified. Anything here that could crash, "
                 "overflow or read out of bounds is listed as a defect instead.</div>")
        P.append("<div class='scroll'><table><tr><th>property</th><th>spec</th><th>what the code does</th>"
                 "<th>why it is not a defect</th></tr>")
        for p_ in dis:
            v_ = discord.get(p_["id"], {})
            P.append(f"<tr><td class='mono'>{esc(p_['id'])}</td>"
                     f"<td>{esc(str(p_.get('statement','')).strip())}</td>"
                     f"<td>{esc(str(v_.get('code_does','')))}</td>"
                     f"<td>{esc(str(v_.get('why','')))}</td></tr>")
        P.append("</table></div>")

    if tbs:
        P.append(f"<h2>{sec} · Tool-boundary rows and their causes</h2>")
        sec += 1
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

    if notes_html:
        P.append("<h2 id='notes'>Corrections and notes</h2>")
        P.extend(notes_html)

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

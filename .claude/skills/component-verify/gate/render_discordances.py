#!/usr/bin/env python3
"""Render <component>_discordances.html from discordances.yaml (level1.py). Self-contained, small.

A colleague who has never seen the pipeline must understand it: what level 1 is, how many
discordances were found, and for each one what the specification says, what the code does, and
where. Nothing about provers.

usage: render_discordances.py <verif_dir> [--out PATH]
"""
import argparse, html, os, sys
import yaml

esc = lambda s: html.escape(str(s if s is not None else ""))
KIND = {"spec-and-code-differ": "spec and code differ",
        "spec-not-found-in-code": "spec item not found in code"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("verif_dir")
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    d = yaml.safe_load(open(os.path.join(a.verif_dir, "discordances.yaml")))
    comp = d.get("component", "component")
    out = a.out or os.path.join(a.verif_dir, f"{comp}_discordances.html")
    c = d.get("counts", {})
    rows = d.get("discordances") or []
    P = [f"<title>{esc(comp)} discordances</title>", """<style>
:root{--bg:#fbfaf7;--fg:#1d1d1b;--muted:#6b6a64;--line:#e4e1d8;--chip:#efece4}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--bg:#1b1b19;--fg:#ecebe6;--muted:#a3a29b;--line:#3a3934;--chip:#2a2a27}}
:root[data-theme="dark"]{--bg:#1b1b19;--fg:#ecebe6;--muted:#a3a29b;--line:#3a3934;--chip:#2a2a27}
body{background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,-apple-system,Segoe UI,sans-serif;margin:0}
.wrap{max-width:1100px;margin:0 auto;padding:24px 16px 48px}
h1{font-size:22px;margin:0 0 6px} .sub{color:var(--muted);margin:0 0 18px}
.scroll{overflow-x:auto} table{border-collapse:collapse;width:100%;font-size:14px}
th,td{border-top:1px solid var(--line);padding:8px 10px;vertical-align:top;text-align:left}
th{font-weight:600;color:var(--muted);font-size:13px} code{font-size:12.5px}
.chip{background:var(--chip);border-radius:4px;padding:1px 6px;font-size:12px;white-space:nowrap}
.ptr{color:var(--muted);font-size:12.5px}
</style><div class='wrap'>""",
         f"<h1>{esc(comp)} — spec↔code discordances</h1>",
         f"<p class='sub'>Pin <code>{esc(d.get('pin',''))}</code> · Level 1 of verification</p>",
         "<p>Before any formal proof, the component's specification and its code are each read "
         "independently and the two readings are compared. Where they <b>disagree</b>, or the "
         "specification asks for something not found in the code, it is listed here. These are "
         "findings of the verification, but they are <b>not formally verified</b>: proving them would "
         "only rediscover what is already plain from the text. Whoever keeps the specification and the "
         "code in step decides which side to change. Each entry is a <i>candidate</i> until a test "
         "confirms it.</p>",
         f"<p><b>{c.get('discordances', len(rows))}</b> discordances: "
         f"{c.get('spec_and_code_differ', 0)} where spec and code differ, "
         f"{c.get('spec_not_found_in_code', 0)} where a spec item was not found in the code.</p>"]
    if not rows:
        P.append("<p>None — the specification and the code agree everywhere they were compared.</p>")
    else:
        P.append("<div class='scroll'><table><tr><th>#</th><th>kind</th><th>method</th>"
                 "<th>what the specification says</th><th>what the code does</th><th>where</th>"
                 "<th>status</th></tr>")
        for i, e in enumerate(rows, 1):
            where = ("<b>spec</b> " + esc(", ".join(e.get("spec_pointers") or []) or "—") +
                     "<br><b>code</b> " + esc(", ".join(e.get("code_pointers") or []) or "—"))
            P.append(f"<tr><td>{i}</td><td><span class='chip'>{esc(KIND.get(e.get('kind'), e.get('kind')))}</span></td>"
                     f"<td><code>{esc(', '.join(e.get('methods') or []))}</code></td>"
                     f"<td>{esc(e.get('spec_says'))}</td><td>{esc(e.get('code_does'))}</td>"
                     f"<td class='ptr'>{where}<br><code>{esc(e.get('id'))}</code></td>"
                     f"<td>{esc(e.get('status'))}</td></tr>")
        P.append("</table></div>")
    P.append("</div>")
    open(out, "w", encoding="utf-8").write("\n".join(P))
    print(f"render_discordances: wrote {out} ({len(rows)} rows)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Merge two INDEPENDENT level-1 classifications (classify_rubric.yaml) into the bundle.

Each classifier agent writes verif/classify_<run>.yaml:  {<property id>: {class: A|B|C, reason: <C reason>,
checked_by: <tool>, evidence: [file:line...], why: <one sentence>}}. Where the two runs agree on the class
(and on the C reason), the result is written as `classification:` into unified_properties.yaml. Where they
disagree, the property gets class `?` and `needs_human: true` - it stays a discordance until a person decides.
A and B without code evidence are rejected (treated as disagreement). Exit 0 always; prints a table.

usage: classify_merge.py <verif_dir> <run1.yaml> <run2.yaml>
"""
import os, sys
import yaml

def main():
    vd, r1, r2 = sys.argv[1], sys.argv[2], sys.argv[3]
    a = yaml.safe_load(open(r1)) or {}
    b = yaml.safe_load(open(r2)) or {}
    bp = os.path.join(vd, "unified_properties.yaml")
    d = yaml.safe_load(open(bp))
    out, rows = {}, []
    for pid in sorted(set(a) | set(b)):
        x, y = a.get(pid) or {}, b.get(pid) or {}
        ok = lambda v: v.get("class") in ("A", "B", "C") and (v.get("class") == "C" or v.get("evidence"))
        agree = ok(x) and ok(y) and x["class"] == y["class"] and (x["class"] != "C" or x.get("reason") == y.get("reason"))
        if agree:
            out[pid] = {"class": x["class"], "reason": x.get("reason"),
                        "checked_by": x.get("checked_by") if x.get("checked_by") == y.get("checked_by") else x.get("checked_by"),
                        "evidence": list(dict.fromkeys((x.get("evidence") or []) + (y.get("evidence") or []))),
                        "why": x.get("why")}
        else:
            out[pid] = {"class": "?", "needs_human": True,
                        "run1": {k: x.get(k) for k in ("class", "reason", "evidence", "why")},
                        "run2": {k: y.get(k) for k in ("class", "reason", "evidence", "why")}}
        rows.append((pid, out[pid]["class"], out[pid].get("reason") or "", x.get("class"), y.get("class")))
    d["classification"] = out
    yaml.safe_dump(d, open(bp, "w"), sort_keys=False, allow_unicode=True, width=110)
    for r in rows:
        print(f"{r[1]:2} {r[0]:60} {r[2]:13} (run1 {r[3]}, run2 {r[4]})")
    n = lambda c: sum(1 for v in out.values() if v["class"] == c)
    print(f"classify_merge: A {n('A')} | B {n('B')} | C {n('C')} | needs a person {n('?')}")
    return 0

if __name__ == "__main__":
    sys.exit(main())

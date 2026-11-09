#!/usr/bin/env python3
"""CROSS-TOOL CHECK — run after both scorers. One tool PROVED a property and the other REFUTED it.

Both cannot be right about the same obligation, so one of them checked a different statement: a proof
that assumed something the obligation does not say (promised less), or a refutation that started from a
state the component can never reach (one that breaks a proved invariant). Measured on dispatch-map
2026-10-06: four such pairs in one run, two of each kind; neither scorer alone could see them.
Exit 1 lists every pair; the run is not done until each is resolved at the artifact, never by editing
a status.

usage: cross_check.py <verif_dir> [--yaml unified_properties.yaml]
"""
import argparse, os, sys
import yaml


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("verif_dir")
    ap.add_argument("--yaml", default="unified_properties.yaml")
    a = ap.parse_args()
    d = yaml.safe_load(open(os.path.join(a.verif_dir, a.yaml)))
    ex = set(d.get("level2_excluded") or [])
    bad = []
    for p in d.get("properties", []):
        if not p.get("verifiable") or p.get("id") in ex:
            continue
        st = {t: (p.get(t) or {}).get("status") for t in ("creusot", "kani")}
        if {"proved", "refuted"} <= set(st.values()):
            bad.append((p["id"], st))
    for pid, st in bad:
        print(f"CROSS-TOOL CONTRADICTION {pid}: creusot={st['creusot']} kani={st['kani']}")
    print(f"cross_check: {len(bad)} contradiction(s)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())

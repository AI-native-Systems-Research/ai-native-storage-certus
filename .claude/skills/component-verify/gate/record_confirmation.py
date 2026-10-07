#!/usr/bin/env python3
"""Record that a test CONFIRMED or WITHDREW a level-1 discordance (candidate -> confirmed / withdrawn).

The decision is made by a test, never by reading (Cornel, 2026-10-05): the repair agent's step 1 writes a
test of the specification's requirement and runs it on the code BEFORE any fix (repair_accept.sh leg 2b,
RED-FIRST).
  confirmed  the test FAILS on that code: the code really does not do what the spec says.
  withdrawn  the test PASSES: the code does it and the code reader missed it.
Writes <verif_dir>/discordance_confirmations.yaml; level1.py applies it every time it regenerates
discordances.yaml, so the result is not lost on re-extraction.

usage: record_confirmation.py <verif_dir> <discordance-id> confirmed|withdrawn --evidence TEXT
                              [--property PROP-ID] [--fix "PR #505"] [--date YYYY-MM-DD]
"""
import argparse, datetime, os, sys
import yaml


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("verif_dir")
    ap.add_argument("id")
    ap.add_argument("status", choices=["confirmed", "withdrawn"])
    ap.add_argument("--evidence", required=True, help="which test, run on which revision, what it showed")
    ap.add_argument("--property", default=None, help="excluded property id (fallback match if the id shifts)")
    ap.add_argument("--fix", default=None)
    ap.add_argument("--date", default=datetime.date.today().isoformat())
    a = ap.parse_args()
    dpath = os.path.join(a.verif_dir, "discordances.yaml")
    rows = (yaml.safe_load(open(dpath)) or {}).get("discordances") or []
    row = next((r for r in rows if r["id"] == a.id), None)
    if row is None:
        print(f"record_confirmation: no discordance {a.id} in {dpath}", file=sys.stderr)
        return 2
    fp = os.path.join(a.verif_dir, "discordance_confirmations.yaml")
    doc = (yaml.safe_load(open(fp)) if os.path.exists(fp) else None) or {}
    conf = [c for c in doc.get("confirmations") or [] if c.get("id") != a.id]
    c = {"id": a.id, "property": a.property or (row.get("excluded_properties") or [None])[0],
         "status": a.status, "evidence": a.evidence, "date": a.date}
    if a.fix:
        c["fix"] = a.fix
    conf.append(c)
    doc = {"what": "Test results for level-1 discordances (record_confirmation.py). confirmed = a test of the "
                   "spec's requirement FAILED on the code before any fix; withdrawn = it PASSED (the code "
                   "reader missed it). Applied by level1.py on every regeneration.",
           "confirmations": conf}
    yaml.safe_dump(doc, open(fp, "w"), sort_keys=False, allow_unicode=True, width=110)
    print(f"record_confirmation: {a.id} -> {a.status}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

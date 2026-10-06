#!/usr/bin/env python3
"""LEVEL 1 — spec<->code reconciliation. Writes discordances.yaml and the level-2 exclusion list.

Verification has two levels (Cornel, 2026-10-04):
  Level 1  reconciling what the specification says with what the code says. Cheap, no provers.
           Finds DISCORDANCES: the two sides say different things, or the spec requires something
           the code does not do. These are a verification RESULT, reported here, and they are
           NOT fed to the provers: refuting a discordance only re-discovers it (measured: 43 of
           46 published "refutations" were exactly that).
  Level 2  formal verification of what remains: properties both sides agree on, plus the code's
           own documented guarantees (code-only). Finds the bugs reading cannot see.

Every property derived from a discordance — from EITHER side — is excluded from level 2. Which side
is wrong does not matter here; whoever owns spec/code synchronisation decides, or the repair agent
confirms the discordance with a test that fails on today's code. Once a discordance is fixed, the
next extraction sees agreement and the property enters level 2 by itself.

usage: level1.py <verif_dir> [--yaml unified_properties.yaml] [--dry-run]
Writes  <verif_dir>/discordances.yaml
Appends `level2_excluded:` to the bundle (text append; scorer-written bytes are never rewritten).
"""
import argparse, io, os, re, sys, hashlib
import yaml

LEVEL1_ORIGINS = {"divergent": "spec-and-code-differ", "spec-only": "spec-not-found-in-code", "spec": "spec-not-found-in-code"}
SPEC_PTR = re.compile(r"^(FR|NFR|US|AS|SC|SA|EC|CLAR|ASSUMPTION|contract|plan\.md)[-_: A-Za-z0-9.]*", re.I)
CODE_PTR = re.compile(r"([A-Za-z0-9_]+\.rs)(?::\d+(?:-\d+)?)?")


def _strs(x):
    if x is None:
        return []
    if isinstance(x, str):
        return [x]
    if isinstance(x, dict):
        return [s for v in x.values() for s in _strs(v)]
    if isinstance(x, (list, tuple)):
        return [s for v in x for s in _strs(v)]
    return [str(x)]


def pointers(p):
    """(spec pointers, code pointers) from traces / source / object_fn, de-duplicated, in order."""
    raw = _strs(p.get("traces")) + _strs(p.get("source"))
    spec, code = [], []
    for r in raw:
        r = re.sub(r"^(spec|code):\s*", "", r.strip())
        for part in re.split(r",\s*", r):
            part = part.strip()
            if not part:
                continue
            if CODE_PTR.search(part):
                code.append(CODE_PTR.search(part).group(0))
            elif SPEC_PTR.match(part):
                spec.append(part)
    of = str(p.get("object_fn") or "")
    m = CODE_PTR.search(of)
    if m:
        code.insert(0, m.group(0))
    return list(dict.fromkeys(spec)), list(dict.fromkeys(code))


def stable_id(comp, p, spec, code):
    """Same discordance -> same id across extraction runs, as far as the pointers allow: keyed on the
    first spec pointer and the first code location, never on the extraction's own record id."""
    s = (spec[0] if spec else "nospec").upper().replace(" ", "")
    c = code[0] if code else (str(p.get("object_fn") or "nocode").split("->")[0].strip() or "nocode")
    h = hashlib.sha1(f"{comp}|{s}|{c}|{p.get('kind','')}".encode()).hexdigest()[:6]
    return f"D-{s}-{re.sub(r'[^A-Za-z0-9.:_]', '', c)}-{h}"


ROLE_TAGS = {"POST", "PRE", "INV", "FRAME"}


def readable_name(pids, methods):
    """A short name a reader can recognise ("Inv stale handle never crashes" -> "Stale handle never crashes"),
    made from the property id the extraction gave it: drop the component prefix and the POST/PRE/INV/FRAME
    tag, keep the method words (stripping them left a dozen rows called just "Precondition").
    Cornel, 2026-10-05: the page's "kind" column meant nothing to a reader; the property name is what
    tells you which row is the stale-handle one."""
    names = []
    for pid in pids:
        toks = [t for t in str(pid).split("-")[1:] if t not in ROLE_TAGS - {"PRE"}] or str(pid).split("-")[1:]
        toks = ["error" if t == "ERR" else ("precondition" if t == "PRE" else t.lower()) for t in toks]
        n = " ".join(toks) or str(pid)
        names.append(n[:1].upper() + n[1:])
    return "; ".join(dict.fromkeys(names))


def load_confirmations(verif_dir):
    """discordance_confirmations.yaml (record_confirmation.py) survives every regeneration of
    discordances.yaml: a test that confirmed or withdrew a discordance stays on the record."""
    fp = os.path.join(verif_dir, "discordance_confirmations.yaml")
    try:
        return (yaml.safe_load(open(fp)) or {}).get("confirmations") or []
    except OSError:
        return []


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("verif_dir")
    ap.add_argument("--yaml", default="unified_properties.yaml")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    bpath = os.path.join(a.verif_dir, a.yaml) if not os.path.isabs(a.yaml) else a.yaml
    d = yaml.safe_load(open(bpath))
    comp = d.get("component", "component")
    # EACH SIDE IN ITS OWN WORDS (Cornel, 2026-10-04): "what the specification says" comes from the blind
    # SPEC reader's statement and "what the code does" from the blind CODE reader's, both plain English
    # and written independently. The unified record's statement is NOT used for either: on
    # eviction-policy-optimized it was "adjudicated in favour of the CODE", so it showed the code's
    # version under "spec says", and the code column was empty.
    def _stmts(fname):
        fp = os.path.join(os.path.dirname(bpath), fname)
        try:
            recs = (yaml.safe_load(open(fp)) or {}).get("properties") or []
        except OSError:
            return {}
        return {r["id"]: re.sub(r"\s+", " ", str(r.get("statement", ""))).strip()
                for r in recs if r.get("id") and r.get("statement")}
    SPEC_TXT, CODE_TXT = _stmts("spec_properties.yaml"), _stmts("code_properties.yaml")
    disc, excluded = [], []
    for p in d.get("properties", []):
        if not p.get("verifiable"):
            continue
        kind = LEVEL1_ORIGINS.get(str(p.get("origin", "")))
        if not kind:
            continue
        spec, code = pointers(p)
        pf = p.get("paired_from") or p.get("derived_from") or {}      # field name varies by run
        sides = {"spec": _strs(pf.get("spec")) if isinstance(pf, dict) else [],
                 "code": _strs(pf.get("code")) if isinstance(pf, dict) else []}
        spec_txt = " ".join(dict.fromkeys(SPEC_TXT[i] for i in sides["spec"] if i in SPEC_TXT))
        code_txt = " ".join(dict.fromkeys(CODE_TXT[i] for i in sides["code"] if i in CODE_TXT))
        entry = {
            "id": stable_id(comp, p, spec, code),
            "name": readable_name([p["id"]], list(p.get("methods") or [])),
            "kind": kind,
            "methods": list(p.get("methods") or []),
            "spec_says": spec_txt or re.sub(r"\s+", " ", str(p.get("statement", ""))).strip(),
            "code_does": ("Nothing found." if kind == "spec-not-found-in-code" else
                          code_txt or re.sub(r"\s+", " ", str(p.get("divergence_note") or p.get("note") or "")).strip()),
            "spec_pointers": spec,
            "code_pointers": code,
            "status": "candidate",
            "excluded_properties": [p["id"]],
            "extraction_sides": sides,
        }
        disc.append(entry)
        excluded.append(p["id"])
    # INPUT-RANGE MISMATCHES (the level-1 sync check, build-property-inventory step 2): one entry each,
    # however many properties depend on them. Those properties stay in level 2 and are proved UNDER the
    # narrower range (`assume`), so the mismatch is reported once here instead of rediscovered as
    # dozens of refutations (extent-manager: 28 of 75 refutations were one such mismatch).
    assumptions = []
    for k, dd in enumerate(d.get("domain_discordances") or [], 1):
        sp = list(dd.get("spec_pointers") or []); cp = list(dd.get("code_pointers") or [])
        h = hashlib.sha1(f"{comp}|{'|'.join(sp)}|{'|'.join(cp)}|domain".encode()).hexdigest()[:6]
        did = f"D-RANGE-{(sp[0] if sp else 'nospec').upper().replace(' ', '')}-{h}"
        disc.append({"id": did, "name": dd.get("name") or "Input range: " + str(dd.get("assume", "")).strip(),
                     "kind": "input-range-mismatch", "methods": list(dd.get("methods") or []),
                     "spec_says": dd.get("spec_says", ""), "code_does": dd.get("code_does", ""),
                     "spec_pointers": sp, "code_pointers": cp, "status": "candidate",
                     "assume_in_level2": dd.get("assume", ""),
                     # a property whose premise IS the excluded range would only be proved vacuously under
                     # the assumption (dispatch-map 2026-10-06: DM-INITIALIZE-PRESERVES-REFERENCED-ENTRY
                     # under D-RANGE-FR-020) -> it belongs to this discordance, not to level 2
                     "excluded_properties": list(dd.get("excluded_properties") or [])})
        excluded.extend(x for x in (dd.get("excluded_properties") or []) if x not in excluded)
        assumptions.append({"id": did, "assume": dd.get("assume", ""), "assume_rust": dd.get("assume_rust", ""),
                            "methods": list(dd.get("methods") or [])})
    # UNREACHABLE (dispatch-map 2026-10-06): a property whose premise can never hold in any state that
    # satisfies the component's PROVED invariants. Proving it is vacuous (no mutant can fail) and
    # refuting it needs an impossible starting state, so it is neither proved nor a defect. Listed by the
    # orchestrator under `level2_unreachable:` with the reason; excluded from level 2, not a discordance.
    for u in d.get("level2_unreachable") or []:
        if u.get("id") and u["id"] not in excluded:
            excluded.append(u["id"])
    out = {
        "component": comp,
        "pin": d.get("pin"),
        "level": 1,
        "what": "spec<->code discordances found while reconciling the specification with the code. "
                "Each is EXCLUDED from formal verification (level 2). status: candidate until a test "
                "confirms it (confirmed) or shows it was a misreading (withdrawn).",
        "counts": {"discordances": len(disc),
                   "spec_and_code_differ": sum(1 for e in disc if e["kind"] == "spec-and-code-differ"),
                   "spec_not_found_in_code": sum(1 for e in disc if e["kind"] == "spec-not-found-in-code"),
                   "input_range_mismatch": len(assumptions),
                   "properties_excluded_from_level2": len(excluded)},
        "discordances": disc,
    }
    # CONFIRMATIONS: candidate -> confirmed / withdrawn, matched by discordance id, else by property id
    # (an id can shift if a pointer changes; the property name usually does not).
    conf = load_confirmations(a.verif_dir)
    used = set()
    for e in disc:
        for k, c in enumerate(conf):
            if c.get("id") == e["id"] or (c.get("property") and c["property"] in e.get("excluded_properties", [])):
                e["status"] = c.get("status", "candidate")
                for f in ("evidence", "fix", "date"):
                    if c.get(f):
                        e[f"status_{f}"] = c[f]
                used.add(k)
                break
    for k, c in enumerate(conf):
        if k not in used:
            print(f"level1: note: confirmation {c.get('id') or c.get('property')} matches no current discordance "
                  "(fixed and re-extracted as agreement?)", file=sys.stderr)
    out["counts"]["confirmed"] = sum(1 for e in disc if e["status"] == "confirmed")
    out["counts"]["withdrawn"] = sum(1 for e in disc if e["status"] == "withdrawn")
    ids = [e["id"] for e in disc]
    dup = {i for i in ids if ids.count(i) > 1}
    if dup:
        print(f"level1: WARNING {len(dup)} id collision(s), disambiguated", file=sys.stderr)
        seen = {}
        for e in disc:
            if e["id"] in dup:
                seen[e["id"]] = seen.get(e["id"], 0) + 1
                e["id"] = f"{e['id']}-{seen[e['id']]}"
    print(f"level1: {comp}: {len(disc)} discordances "
          f"({out['counts']['spec_and_code_differ']} differ, {out['counts']['spec_not_found_in_code']} spec-not-found-in-code); "
          f"{len(assumptions)} input-range mismatch(es); {len(excluded)} properties excluded from level 2")
    if a.dry_run:
        return 0
    yaml.safe_dump(out, open(os.path.join(a.verif_dir, "discordances.yaml"), "w"),
                   sort_keys=False, allow_unicode=True, width=110)
    txt = io.open(bpath, encoding="utf-8").read()
    lines = txt.split("\n")
    for key in ("level2_assumptions:",):
        if any(l.startswith(key) for l in lines):
            st = next(i for i, l in enumerate(lines) if l.startswith(key))
            en = next((i for i in range(st + 1, len(lines)) if re.match(r"^[A-Za-z_#]", lines[i])), len(lines))
            if st > 0 and lines[st - 1].startswith("# LEVEL 1: input-range"):
                st -= 1
            del lines[st:en]
    if any(l.startswith("level2_excluded:") for l in lines):     # idempotent: replace the old block
        st = next(i for i, l in enumerate(lines) if l.startswith("level2_excluded:"))
        en = next((i for i in range(st + 1, len(lines)) if re.match(r"^[A-Za-z_#]", lines[i])), len(lines))
        if st > 0 and lines[st - 1].startswith("# LEVEL 1"):
            st -= 1
        del lines[st:en]
        txt = "\n".join(lines)
    txt = txt.rstrip("\n") + ("\n# LEVEL 1 (level1.py): properties derived from a spec<->code discordance; see "
                              "discordances.yaml. Never sent to the provers.\n")
    txt += yaml.safe_dump({"level2_excluded": excluded}, sort_keys=False, width=200)
    if assumptions:
        txt += ("# LEVEL 1: input-range mismatches. Level 2 proves the dependent properties UNDER these.\n"
                + yaml.safe_dump({"level2_assumptions": assumptions}, sort_keys=False, allow_unicode=True, width=200))
    io.open(bpath, "w", encoding="utf-8").write(txt)
    return 0


if __name__ == "__main__":
    sys.exit(main())

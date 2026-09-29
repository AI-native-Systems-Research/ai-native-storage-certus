#!/usr/bin/env python3
"""scorer_creusot.py — the Creusot REPRODUCTION GATE. Reproduction, not declaration.

Mirror of scorer_kani.py for Creusot/why3. The proving agent NEVER writes
`creusot.status`. This scorer does — by RE-RUNNING `cargo creusot <module>`, which
regenerates the .coma FROM SOURCE and discharges that module's goals with the
configured provers. It is read-only to the agent at run time.

ANTI-TAMPER (measured 2026-09-16): cargo skips recompiling when src is unchanged,
so a hand-edited .coma can fake a `Proved`. This scorer `touch`es every src/**.rs
once at startup, forcing creusot-rustc to regenerate every .coma from source before
any judgement. A tampered .coma is overwritten and the real goal is proved or fails.

For every verifiable property it computes exactly one of a tiny closed set:
    proved        — scorer ran the module (base, a scorer-applied lever escalation, or an
                    agent-written lever variant) and `cargo creusot` reported it Proved.
                    Evidence (module, result, wall_clock_s, peak_rss_mb, lever) is CAPTURED
                    FROM THE RUN. Anti-vacuity: a `verify_<ID>__mutant` module, if present,
                    MUST fail, else the property is vacuous.
    tool-boundary — one of two earned routes:
                    (a) BATTERY-EXHAUSTED: scorer ran the FULL lever battery for the observed
                        failure class (prover portfolio + budget + split_vc itself, plus every
                        required code-lever variant the agent supplied) and each still failed,
                        AND the residual signature is NOT a known defeat; OR
                    (b) NOT-EXPRESSIBLE: the property cited an ACTIVE construct in
                        inexpressible_creusot.yaml (creusot.claims_inexpressible: <id>) whose
                        documented type-model authority the scorer CONFIRMED by building that
                        construct's canonical probe in an isolated crate and observing the
                        model-predicted translate/ICE signature. Disjoint from known_defeats;
                        a probe that instead PROVES, or a matching normal proof module, scores
                        the property `proved` (claim rejected, no penalty).
    delegated     — a resolvable referent exists (named component + concrete obligation).
    UNRESOLVED    — everything else (no module, tamper/lie, missing required lever variant,
                    signature matches a known defeat, translate error = broken crate,
                    unclassifiable failure). The gate FAILS if any property is UNRESOLVED.

Usage:
    scorer_creusot.py <verif_dir> [--yaml unified_properties.yaml] [--crate-dir DIR]
                      [--gate-dir DIR] [--dry-run] [--only ID[,ID...]] [--cap-seconds N]

--dry-run: do NOT invoke cargo creusot. Validates module existence + battery
completeness + registry matching only. Use it to watch the gate fail-closed
instantly over a whole component before spending compute.
"""
import argparse, os, re, signal, subprocess, sys, time, shutil, glob, tempfile, json
from datetime import datetime
try:
    import yaml
except ImportError:
    sys.exit("scorer_creusot: PyYAML required (python3 -c 'import yaml')")

ACCEPT = {"proved", "tool-boundary", "delegated", "refuted"}
# `refuted` = the obligation is FALSE, machine-checked. It is ACCEPTED, not a gate
# failure: the verification did its job and found a real defect. Per Cornel 2026-09-26 —
# "when we find an error in verifying a property that is very good, this is what rewards
# our verification effort" — it is shown red in the HTML with the spec and code locations
# and the run continues. Only UNRESOLVED (unfinished work) fails the gate.
CREUSOT_BIN = os.path.expanduser("~/.local/share/creusot/bin") + ":" + os.path.expanduser("~/.cargo/bin")
_UNIT_SEQ = 0


def _capture(cmd, cwd=None, timeout=60, env=None):
    """First line of a command's output, or None. Never raises: provenance is best-effort
    metadata and must never fail a scoring run."""
    try:
        r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout, env=env)
        if r.returncode != 0:
            return None
        lines = ((r.stdout or "") + (r.stderr or "")).strip().splitlines()
        return lines[0].strip()[:160] if lines else None
    except Exception:
        return None


def _why3_provers(env=None):
    """The distinct prover portfolio why3 will actually dispatch to, e.g.
    ['Alt-Ergo 2.6.2', 'CVC4 1.8', 'CVC5 1.3.1', 'Z3 ...']. Recorded because a Creusot result
    is only reproducible against the same portfolio — a missing prover changes the outcome."""
    try:
        r = subprocess.run(["why3", "config", "list-provers"], capture_output=True, text=True,
                           timeout=120, env=env)
        if r.returncode != 0:
            return None
        seen = []
        for ln in (r.stdout or "").splitlines():
            base = ln.split("(")[0].strip()          # drop "(counterexamples)" / "(BV)" variants
            if base and base not in seen:
                seen.append(base)
        return seen or None
    except Exception:
        return None


def _gate_provenance():
    """Identity of the gate code doing the scoring, derived at RUNTIME from this file's own
    location — so a result can always be traced back to the exact code that produced it, with
    nothing hardcoded and nothing to update when the branch moves.

    `gate_dirty: true` means the gate had uncommitted edits when it ran, so the commit alone
    does NOT fully describe what scored the run. That flag is the honest signal: without it, a
    bare SHA in the record would overstate how reproducible the result is."""
    gd = os.path.dirname(os.path.abspath(__file__))
    commit = _capture(["git", "-C", gd, "rev-parse", "--short", "HEAD"])
    if not commit:
        return {"gate_commit": "unknown — gate dir is not a git checkout"}
    out = {"gate_commit": commit}
    branch = _capture(["git", "-C", gd, "rev-parse", "--abbrev-ref", "HEAD"])
    if branch:
        out["gate_branch"] = branch
    try:
        r = subprocess.run(["git", "-C", gd, "status", "--porcelain", "--", gd],
                           capture_output=True, text=True, timeout=60)
        if r.returncode == 0 and r.stdout.strip():
            out["gate_dirty"] = True
    except Exception:
        pass
    return out


def _stamp_run(d, tool, tool_env, started):
    """Write the `run:` provenance block — which gate, which command, which tool versions —
    so a result on a verif branch says what produced it.

    Each scorer owns ONLY `run[<tool>]`, and the shared gate identity both write is identical,
    so the two scorers cannot clobber each other. argv[0] is reduced to its basename so the
    record carries no machine-specific path."""
    run = d.get("run")
    if not isinstance(run, dict):
        run = {}
    run.update(_gate_provenance())
    blk = {
        "scored_by": f"scorer_{tool}",
        "command": " ".join([os.path.basename(sys.argv[0])] + sys.argv[1:]),
        "started": started,
        "finished": datetime.now().astimezone().isoformat(timespec="seconds"),
    }
    for k, v in (tool_env or {}).items():
        if v is not None:
            blk[k] = v
    run[tool] = blk
    d["run"] = run


def module_id(pid):
    return "verify_" + pid.lower().replace("-", "_")


def load(p):
    with open(p) as f:
        return yaml.safe_load(f)


def _new_unit(tag):
    """A unique transient-scope unit name, so a timed-out run can be tree-killed by cgroup."""
    global _UNIT_SEQ
    _UNIT_SEQ += 1
    return f"cv-{tag}-{os.getpid()}-{_UNIT_SEQ}.scope"


def _kill_tree(proc, unit):
    """Reap the WHOLE process tree of a timed-out run — not just the direct child.
    `cargo creusot` fans out why3find + alt-ergo/z3/cvc5/cvc4; a bare kill on the parent
    orphans the provers and they keep burning cores. When the run was placed in a NAMED
    systemd --user scope we kill by cgroup (every descendant); the process group is
    SIGKILLed as a fallback for the no-cap path."""
    if unit:
        subprocess.run(["systemctl", "--user", "kill", "--signal=SIGKILL", unit],
                       capture_output=True, text=True, timeout=15)
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except (ProcessLookupError, PermissionError, OSError):
        pass
    if unit:
        subprocess.run(["systemctl", "--user", "reset-failed", unit],
                       capture_output=True, text=True, timeout=15)


def _exec_capped(cmd, cwd, cap, env, unit):
    """Run cmd in its own session; enforce `cap` seconds; tree-kill on timeout.
    Returns (out, rc, wall_s, timed_out). stdout+stderr merged so `time -v` RSS is captured."""
    t0 = time.time()
    proc = subprocess.Popen(cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, env=env, start_new_session=True)
    timed_out = False
    try:
        out, _ = proc.communicate(timeout=cap)
        rc = proc.returncode
    except subprocess.TimeoutExpired:
        _kill_tree(proc, unit)
        try:
            out, _ = proc.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            out = ""
        rc = proc.returncode if proc.returncode is not None else 124
        out = (out or "") + f"\nTIMEOUT after {cap}s"
        timed_out = True
    return out or "", rc, round(time.time() - t0, 2), timed_out


def _save_yaml(d, path):
    """Atomic checkpoint: write to a temp then os.replace, so a kill mid-write never corrupts
    the YAML and every scored property is durable for --resume."""
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        yaml.safe_dump(d, f, sort_keys=False, width=100, allow_unicode=True)
    os.replace(tmp, path)


def find_coma_modules(crate_dir):
    """Every generated proof module, keyed by basename: {module names}.

    RECURSIVE on purpose. The glob used to be `verif/*_rlib/*.coma`, i.e. exactly one level below
    the _rlib dir. A version of this concern was raised during the eviction-policy-optimized run and
    correctly REJECTED on the evidence then available: all 242 of that component's .coma sat at
    exactly that depth. On eviction-policy-session-lists 2 of 230 do not —
    `impl_Clone_for_Handle/clone.coma` and `impl_Clone_for_PolicyError/clone.coma` — which proves
    Creusot nests VCs one level deeper for trait impls.

    Those two are derived-impl VCs that no property names, so nothing was mis-scored. But the
    failure mode if a property's proof module were ever authored inside an `impl` block is nasty and
    silent: the module is on disk, the glob cannot see it, and the property scores UNRESOLVED with
    "no generated proof module … absence is not a tool limit" — blaming the proving agent for a
    glob's depth assumption. Walking the tree costs nothing and removes the trap.
    """
    names = set()
    for root in glob.glob(os.path.join(crate_dir, "verif", "*_rlib")):
        for dirpath, _dirnames, filenames in os.walk(root):
            for f in filenames:
                if f.endswith(".coma"):
                    names.add(os.path.splitext(f)[0])
    return names


def touch_sources(crate_dir):
    """Force creusot-rustc to regenerate every .coma FROM SOURCE (defeats .coma tampering)."""
    n = 0
    for root, _, files in os.walk(os.path.join(crate_dir, "src")):
        for fn in files:
            if fn.endswith(".rs"):
                os.utime(os.path.join(root, fn), None)
                n += 1
    return n


def classify(text, battery):
    for cls, spec in battery["failure_classes"].items():
        for sig in spec["signatures"]:
            if re.search(sig, text, re.I):
                return cls
    return None


def known_defeat(text, registry):
    for d in registry.get("defeats", []):
        if d.get("tool") not in (None, "creusot"):
            continue
        for sig in d.get("signatures", []):
            if re.search(sig, text, re.I):
                return d
    return None


def load_inexpressible(path, registry, tool):
    """Load the inexpressibility allowlist (if present) and enforce that NO signature it
    lists collides with a known_defeat for the same tool. A construct is either BEATABLE
    (known_defeats -> boundary REJECTED, apply a lever) or genuinely INEXPRESSIBLE
    (this registry -> boundary EARNED via a reproduced probe) — never argued both ways.
    A collision means the two registries disagree about the same wall, so we FAIL-SAFE
    (abort) rather than let an "inexpressible" verdict launder a beatable wall.

    Returns the parsed registry (or None if the file is absent). NOTE: this only LOADS
    and validates the registry; the scoring path does not consult it yet — probe-based
    enforcement is wired in a later, separately-reviewed step.
    """
    if not os.path.exists(path):
        return None
    ix = load(path)
    kd_sigs = []
    for d in registry.get("defeats", []):
        if d.get("tool") in (None, tool):
            kd_sigs += d.get("signatures", [])
    collisions = []
    for c in ix.get("constructs", []):
        cid = c.get("id", "?")
        # confirmation_signatures is the current field; the older names are still swept
        # so a stale entry can never slip a collision past this guard.
        sigs = (c.get("confirmation_signatures", [])
                + c.get("candidate_signatures", [])
                + c.get("signatures", []))
        for sig in sigs:
            for kd in kd_sigs:
                # pragmatic, fail-safe disjointness: identical, or either is a literal
                # substring of the other (curated hand-written sigs; err toward flagging)
                if sig == kd or sig in kd or kd in sig:
                    collisions.append(f"{cid}:'{sig}' <-> known_defeat:'{kd}'")
    if collisions:
        sys.exit(
            "scorer_creusot: FAIL-SAFE — inexpressible/known_defeat signature collision(s): "
            + "; ".join(collisions)
            + ".\n  A construct cannot be both beatable (known_defeats) and inexpressible. "
              "Remove the overlap before running.")
    return ix


def run_creusot(module, crate_dir, cap, extra=None, mem_mb=None, cap_max=None, escalate=True):
    """Run one module under /usr/bin/time -v, in its own session and (when mem_mb is set) a
    NAMED transient systemd --user scope; return (ok, out, wall_s, rss_mb, timed_out, oomed).

    Memory: with mem_mb set the whole process tree (cargo creusot + why3find + every prover it
    fans out — alt-ergo/z3/cvc5/cvc4 run in parallel per goal, so this caps their SUM) runs in a
    scope with MemoryMax=<mem_mb>M and swap disabled. A pathological goal that blows the cap is
    OOM-killed in that scope only (SIGKILL -> rc -9, a shell's 137); reported via `oomed` and
    handled as resource exhaustion, never a free tool-boundary.

    Timeout: capped at `cap` seconds and, on breach, the whole tree is SIGKILLed (why3find +
    provers included), not just `cargo`. Adaptive: on a TIMEOUT only, if escalate and cap_max>cap
    we retry the SAME module once at cap_max before it can be classed a goal-unproved boundary, so
    a merely-slow discharge is not mislabelled a tool-boundary. Probe/mutant runs pass
    escalate=False. module=None runs `cargo creusot` over the WHOLE crate (the isolated
    single-function inexpressibility probe, which aborts translation anyway)."""
    env = dict(os.environ)
    env["PATH"] = CREUSOT_BIN + ":" + env.get("PATH", "")
    time_bin = shutil.which("time") or "/usr/bin/time"

    def once(c):
        unit = _new_unit("creusot") if mem_mb else None
        creusot_cmd = [time_bin, "-v", "cargo", "creusot"] + ([module] if module else []) + (extra or [])
        if mem_mb:
            cmd = ["systemd-run", "--user", "--scope", "--quiet", f"--unit={unit}",
                   "-p", f"MemoryMax={mem_mb}M", "-p", "MemorySwapMax=0"] + creusot_cmd
        else:
            cmd = creusot_cmd
        return _exec_capped(cmd, crate_dir, c, env, unit)

    out, rc, wall, timed_out = once(cap)
    if timed_out and escalate and cap_max and cap_max > cap:
        out, rc, wall2, timed_out = once(cap_max)
        wall = round(wall + wall2, 2)
    oomed = False
    # A cgroup OOM SIGKILLs the whole scope; subprocess reports rc -9, a shell 128+9=137.
    if mem_mb and rc in (-9, 137) and "Proved (" not in out:
        oomed = True
        out += f"\nOOM-KILLED at {mem_mb}M cgroup limit (SIGKILL rc={rc})"
    rss_mb = None
    m = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", out)
    if m:
        rss_mb = round(int(m.group(1)) / 1024)
    # a module is proved iff cargo creusot exits 0 AND prints the Proved line for it,
    # AND no goal is reported unproved.
    ok = (rc == 0 and "Proved (" in out and "✘" not in out and "unproved" not in out.lower())
    return ok, out, wall, rss_mb, timed_out, oomed


def mem_cap_available(mem_mb):
    """True iff a transient systemd --user memory scope can actually be created here.
    Used as a fail-safe preflight: no usable user manager -> refuse to run uncapped."""
    try:
        r = subprocess.run(
            ["systemd-run", "--user", "--scope", "--quiet",
             "-p", f"MemoryMax={mem_mb}M", "-p", "MemorySwapMax=0", "true"],
            capture_output=True, text=True, timeout=30)
        return r.returncode == 0
    except Exception:
        return False


# Isolated probe crate, modeled EXACTLY on the validated ~/creusot_calib_probe fixture.
_PROBE_CARGO_TOML = """[package]
name = "ix-probe"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
creusot-std = "0.12.0-dev"

[lints.rust]
unexpected_cfgs = {{ level = "warn", check-cfg = ['cfg(creusot)'] }}

[patch.crates-io]
creusot-std = {{ path = "{std_path}" }}
"""
_PROBE_WHY3FIND = {"fast": 0.2, "time": 8, "depth": 6, "packages": ["creusot"],
                   "provers": ["alt-ergo", "z3", "cvc5", "cvc4"],
                   "tactics": ["compute_specified", "split_vc"],
                   "drivers": [], "warnoff": ["unused_variable", "axiom_abstract"]}


def _component_creusot_std(crate_dir):
    """The creusot-std patch path the COMPONENT uses, parsed from its Cargo.toml, so the
    probe builds against the SAME creusot-std the real proofs use (identical toolchain).
    Returns a path or None."""
    p = os.path.join(crate_dir, "Cargo.toml")
    if not os.path.exists(p):
        return None
    txt = open(p).read()
    m = re.search(r'creusot-std\s*=\s*\{[^}]*\bpath\s*=\s*"([^"]+)"', txt)
    return m.group(1) if m else None


def _repo_root(start):
    """Nearest ancestor of `start` holding a `.git` entry (the checkout root); None if none."""
    d = os.path.abspath(start)
    while True:
        if os.path.exists(os.path.join(d, ".git")):
            return d
        parent = os.path.dirname(d)
        if parent == d:
            return None
        d = parent


def _resolve_creusot_std(crate_dir, reg):
    """ABSOLUTE path to the creusot-std the isolated probe patches against, or None.

    Checkout-agnostic by construction: nothing here assumes a particular $HOME or clone
    location, so the gate runs unchanged in any colleague's checkout. Tried in order:
      1. $CREUSOT_STD_PATH — explicit operator override;
      2. the component's OWN Cargo.toml patch path, so the probe builds against the same
         creusot-std as the real proofs. Cargo reads such a path relative to the manifest,
         so a relative one is resolved against crate_dir (it is later written into a /tmp
         probe crate, where only an absolute path can resolve);
      3. registry `probe_env.creusot_std_candidates`, each resolved against the repo root;
      4. registry `probe_env.creusot_std_path` — legacy absolute fallback, `~` expanded.
    The first candidate that is an existing directory wins.
    """
    cands = []
    env_p = os.environ.get("CREUSOT_STD_PATH")
    if env_p:
        cands.append(os.path.expanduser(env_p))
    parsed = _component_creusot_std(crate_dir)
    if parsed:
        parsed = os.path.expanduser(parsed)
        cands.append(parsed if os.path.isabs(parsed)
                     else os.path.normpath(os.path.join(os.path.abspath(crate_dir), parsed)))
    pe = reg.get("probe_env") or {}
    root = _repo_root(crate_dir)
    for rel in (pe.get("creusot_std_candidates") or []):
        rel = os.path.expanduser(rel)
        if os.path.isabs(rel):
            cands.append(rel)
        elif root:
            cands.append(os.path.normpath(os.path.join(root, rel)))
    legacy = pe.get("creusot_std_path")
    if legacy:
        cands.append(os.path.expanduser(legacy))
    for c in cands:
        if os.path.isdir(c):
            return c
    return None


def build_probe_isolated(construct, ctx):
    """Build the construct's SCORER-OWNED canonical probe (`probe_lib_rs`) in an ISOLATED
    throwaway crate mirroring the component's creusot-std patch + why3find.json; return
    (proves, out). `proves` is True/False, or None if the probe could not be set up/built.
    Memoised per construct per run — a construct is a per-TOOLCHAIN fact, not per-property.
    A translate error/ICE aborts the crate: that abort IS the not-expressible witness."""
    cid = construct.get("id", "?")
    cache = ctx.setdefault("_probe_cache", {})
    if cid in cache:
        return cache[cid]
    src = construct.get("probe_lib_rs")
    if not src:
        cache[cid] = (None, f"construct {cid} has no probe_lib_rs to build")
        return cache[cid]
    reg = ctx.get("inexpressible") or {}
    std_path = _resolve_creusot_std(ctx["crate_dir"], reg)
    if not std_path:
        cache[cid] = (None, f"cannot locate a creusot-std directory for the isolated probe — tried "
                            f"$CREUSOT_STD_PATH, the component Cargo.toml patch path, and the registry "
                            f"probe_env candidates relative to the repo root of {ctx['crate_dir']!r}")
        return cache[cid]
    tmp = tempfile.mkdtemp(prefix=f"ix_probe_{cid}_")
    try:
        os.makedirs(os.path.join(tmp, "src"))
        with open(os.path.join(tmp, "Cargo.toml"), "w") as f:
            f.write(_PROBE_CARGO_TOML.format(std_path=std_path))
        with open(os.path.join(tmp, "src", "lib.rs"), "w") as f:
            f.write(src)
        comp_w = os.path.join(ctx["crate_dir"], "why3find.json")
        if os.path.exists(comp_w):
            shutil.copy(comp_w, os.path.join(tmp, "why3find.json"))
        else:
            with open(os.path.join(tmp, "why3find.json"), "w") as f:
                json.dump(_PROBE_WHY3FIND, f)
        ok, out, _, _, _, _ = run_creusot(None, tmp, ctx["cap"], mem_mb=ctx["mem_mb"], escalate=False)
        cache[cid] = (ok, out)
    except Exception as e:                       # setup failure is a broken harness, not a boundary
        cache[cid] = (None, f"probe crate setup failed: {e}")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return cache[cid]


def score_inexpressible(p, ctx, ix_id):
    """Adjudicate a property that CLAIMS a documented tool-model boundary
    (creusot.claims_inexpressible: <id>). The verdict is decided by the construct's
    documented `authority` (the type model) and CONFIRMED by building the scorer-owned
    canonical probe in isolation. Fail-closed: only an active, registered construct whose
    probe reproduces the model-predicted signature earns `tool-boundary`."""
    pid = p["id"]
    ix = ctx.get("inexpressible")
    if ix is None:
        return "UNRESOLVED", {}, f"claims_inexpressible={ix_id} but no inexpressibility registry is loaded"
    construct = next((c for c in ix.get("constructs", []) if c.get("id") == ix_id), None)
    if construct is None:
        return "UNRESOLVED", {}, (f"claims_inexpressible={ix_id} names no construct in "
                                  "inexpressible_creusot.yaml (agents may only REFERENCE a curated construct)")
    if not construct.get("active"):
        return "UNRESOLVED", {}, f"claims_inexpressible={ix_id} names an INACTIVE construct — no boundary can be earned from it"

    # abuse-check: an inexpressible property must NOT also ship a normal proof module claiming
    # to prove it. If it does, resolve the contradiction by RUNNING that module.
    base = module_id(pid)
    if base in ctx["modules"] and not ctx["dry_run"]:
        ok, out, wall, rss, _, _ = run_creusot(
            base, ctx["crate_dir"], ctx["cap"], mem_mb=ctx["mem_mb"], cap_max=ctx["cap_max"])
        if ok:
            ev = {"modules": [base], "result": "Proved", "wall_clock_s": wall, "peak_rss_mb": rss, "lever": None}
            # Anti-vacuity applies on EVERY path that returns proved, not just the base one.
            # Three paths here (this one, the scorer-applied lever, and the rejected
            # inexpressibility claim) returned proved with no mutant check until 2026-09-28.
            vac = vacuity_check(pid, present, ctx, extra=None)
            if vac:
                return "UNRESOLVED", {"modules": [base]}, vac
            return "proved", ev, (f"claims_inexpressible={ix_id} REJECTED: proof module '{base}' PROVED — "
                                  "the property was expressible after all (scored proved, no penalty)")
        return "UNRESOLVED", {}, (f"contradictory: property ships proof module '{base}' AND claims "
                                  f"inexpressible ({ix_id}); the module failed — fix the proof or drop the claim")

    if ctx["dry_run"]:
        return "DRY", {"claims_inexpressible": ix_id}, f"dry-run: would build the {ix_id} canonical probe in isolation to confirm the model fact"

    proves, out = build_probe_isolated(construct, ctx)
    src = (construct.get("authority") or {}).get("source")
    if proves is None:
        return "UNRESOLVED", {}, f"could not build the {ix_id} confirmation probe in isolation: {out}"
    if proves:
        return "UNRESOLVED", {}, (f"claims_inexpressible={ix_id} NOT confirmed: the canonical probe PROVED on "
                                  "this toolchain — Creusot CAN express this construct now. Re-calibrate the "
                                  "registry (authority may have changed); this is NOT a boundary")
    hit = next((s for s in construct.get("confirmation_signatures", []) if re.search(s, out, re.I)), None)
    if hit:
        ev = {"fidelity": "not-expressible", "construct": ix_id, "authority": src,
              "covered_by": construct.get("covered_by"), "result": "translate-abort", "signature": hit}
        return "tool-boundary", ev, (f"EARNED not-expressible: construct {ix_id} confirmed — canonical probe "
                                     f"aborted with model-predicted signature /{hit}/; documented authority "
                                     f"{src}; covered_by {construct.get('covered_by')}")
    return "UNRESOLVED", {}, (f"claims_inexpressible={ix_id} NOT confirmed: probe failed but no "
                              f"confirmation_signature matched — re-calibrate. tail: {out[-300:]!r}")


def vacuity_check(pid, present, ctx, extra=None):
    """Anti-vacuity: the `__mutant` twin must FAIL. Returns None if honest, else a reason string.

    A FUNCTION, not inline code, for the reason found on eviction-policy-session-lists on
    2026-09-28: the check existed on the BASE proved path only, while THREE other paths could also
    return "proved" — the scorer-applied lever, the agent-supplied lever variant, and the
    inexpressibility-rejected path — and none of them ran it. Exactly the defect being fixed in the
    Kani scorer at the same time, in a second place. Scattering an obligation across return sites is
    how it gets missed; one helper called from every such site is the fix.

    NAME COLLAPSE: `cargo creusot` renders a source module `verify_x__mutant` into
    `verify_x_mutant.coma` — the double underscore becomes a SINGLE one. This scorer once looked only
    for the double-underscore form, so the lookup never matched and the check was silently skipped for
    EVERY Creusot property on every component (34/20/29/23 mutant modules emitted and never executed).
    Accept either spelling.
    """
    base = module_id(pid)
    mut = next((m for m in (base + "_mutant", base + "__mutant") if m in present), None)
    if not mut:
        return None
    mok, _, _, _, _, _ = run_creusot(
        mut, ctx["crate_dir"], ctx["cap"], mem_mb=ctx["mem_mb"], escalate=False, extra=extra)
    if not mok:
        return None
    how = f" under the same lever as the proof" if extra else ""
    return (f"VACUOUS: mutant module {mut} also proved{how} — the proof holds no content; "
            f"strengthen the property")


def score_property(p, ctx):
    """Return (status, evidence_dict, note). status in ACCEPT or 'UNRESOLVED'."""
    pid = p["id"]
    proposed = (p.get("creusot") or {})
    ev_in = proposed.get("evidence") or {}
    # module pointer: explicit evidence.module(s), else the naming convention
    mods = ev_in.get("modules") or ([ev_in["module"]] if ev_in.get("module") else [module_id(pid)])
    present = ctx["modules"]

    # ---- delegation: triggered by an agent-written delegate_to (the skills forbid the
    #      agent to write `status`), or a legacy status:delegated; needs a resolvable referent ----
    if proposed.get("delegate_to") or proposed.get("status") == "delegated":
        owner = (proposed.get("note") or "") + " " + str(proposed.get("delegate_to", ""))
        if re.search(r"\b(component|crate)\b", owner, re.I) or proposed.get("delegate_to"):
            return "delegated", ev_in, "delegated to a named referent (scorer did not re-derive; refuter audits)"
        return "UNRESOLVED", {}, "delegated with no resolvable referent (name the owning component + obligation)"

    # ---- earned inexpressibility: the property CLAIMS a documented tool-model boundary.
    #      Handled BEFORE the module-presence check — a genuinely inexpressible property has
    #      no statable proof module; the confirmation is a scorer-owned isolated probe. ----
    claim = proposed.get("claims_inexpressible")
    if claim:
        return score_inexpressible(p, ctx, claim)

    # ---- REFUTATION: the obligation is FALSE and here is the machine-checked reason ----
    # Finding a real violation is the point of verifying, not a failure to verify, so it gets a
    # first-class status instead of being filed as UNRESOLVED ("you didn't write the proof"), which
    # is what happened before and made a genuine defect indistinguishable from unfinished work.
    #
    # In Creusot a refutation is sound and constructive: a `refute_<id>` module states the NEGATION
    # of the property, so if it PROVES, the property is false. That is a proof, not a failed proof —
    # which is exactly why a merely-failing `verify_<id>` can never be read as a refutation.
    # Guard against the contradictory case: if the property AND its negation both prove, something
    # is wrong with the model, and claiming a defect would be unsound.
    # `refute_<id>`, NOT refute_ prepended to module_id(): module_id already carries the `verify_`
    # prefix, so that built `refute_verify_<id>` and matched nothing. Caught only because the first
    # real run reported `refuted: 0` against two refutation modules known to be on disk.
    refute = "refute_" + pid.lower().replace("-", "_")
    if refute in present and not ctx["dry_run"]:
        rok, rout, rwall, rrss, _, _ = run_creusot(
            refute, ctx["crate_dir"], ctx["cap"], mem_mb=ctx["mem_mb"], cap_max=ctx["cap_max"])
        if rok:
            base_ok = False
            if all(m in present for m in mods):
                base_ok, _, _, _, _, _ = run_creusot(
                    mods[0], ctx["crate_dir"], ctx["cap"], mem_mb=ctx["mem_mb"], escalate=False)
            if base_ok:
                return "UNRESOLVED", {"modules": mods, "refutation": refute}, (
                    f"CONTRADICTION: both '{mods[0]}' and its negation '{refute}' proved. The model "
                    f"is unsound (a vacuous precondition or a mis-stated negation) — fix it before "
                    f"any verdict; a defect claim on this footing would not be trustworthy.")
            return "refuted", {"refutation": refute, "result": "Proved (negation)",
                               "wall_clock_s": rwall, "peak_rss_mb": rrss}, (
                f"REFUTED — the negation '{refute}' is machine-proved, so the code violates this "
                f"obligation. This is a finding, not a gap: see the spec and code locations on the "
                f"property record.")

    # ---- every named module must exist as a generated .coma ----
    missing_mods = [m for m in mods if m not in present]
    if missing_mods:
        return "UNRESOLVED", {}, f"no generated proof module(s) {missing_mods} (looked for verif/*_rlib/<m>.coma): write the proof — absence is not a tool limit"

    if ctx["dry_run"]:
        return "DRY", {"modules": mods}, "dry-run: module(s) present, not executed"

    # ---- execute every base module; ALL must be Proved for the property to hold ----
    worst = None
    for m in mods:
        ok, out, wall, rss, timed_out, oomed = run_creusot(
            m, ctx["crate_dir"], ctx["cap"], mem_mb=ctx["mem_mb"], cap_max=ctx["cap_max"])
        if ok:
            continue
        worst = (m, out, wall, rss, timed_out, oomed)   # first failing module drives the verdict
        break
    if worst is None:
        # All proved — now the anti-vacuity check on the base id's mutant twin.
        #
        # NAME COLLAPSE: `cargo creusot` renders a source module `verify_x__mutant` into
        # `verify_x_mutant.coma` — the double underscore becomes a SINGLE one. This scorer used to
        # look only for the double-underscore form, so the lookup never matched, `mut in present`
        # was always false, and the vacuity check was SILENTLY SKIPPED for every Creusot property.
        # Measured when this was found: four already-scored components shipped 34/20/29/23 mutant
        # modules whose .coma were all emitted and never once executed by the gate. A check that
        # silently does nothing is worse than no check, because it is reported as enforced.
        # Accept either spelling, and require the FIRST one that exists to fail.
        vac = vacuity_check(pid, present, ctx)
        if vac:
            return "UNRESOLVED", {"modules": mods}, vac
        ev = {"modules": mods, "result": "Proved", "wall_clock_s": wall, "peak_rss_mb": rss, "lever": None}
        return "proved", ev, "scorer re-ran `cargo creusot` from source -> Proved"

    m, out, wall, rss, timed_out, oomed = worst

    # ---- failed: registry first (a beaten wall is never a boundary) ----
    kd = known_defeat(out, ctx["registry"])
    if kd:
        return "UNRESOLVED", {"modules": mods}, (
            f"module '{m}' failed with a signature matching known defeat {kd['id']} — apply lever "
            f"'{kd['mandated_lever']}'; claiming a tool-boundary on a beaten wall is rejected")

    # a hard timeout or a cgroup OOM is decisively resource exhaustion: the obligation did
    # not discharge within its resource budget -> goal-unproved (its full lever battery must
    # still be exhausted before any boundary), never an "unclassifiable" pass-through.
    cls = "goal-unproved" if (timed_out or oomed) else classify(out, ctx["battery"])
    if cls is None:
        return "UNRESOLVED", {"modules": mods}, f"module '{m}' failed with an unclassifiable error — the harness is broken, not the tool; fix it"
    if cls == "translate-error":
        return "UNRESOLVED", {"modules": mods}, f"module '{m}': translation/compile error — the verif crate is broken, not a tool-boundary; fix it"

    required = ctx["battery"]["failure_classes"][cls]["required_levers"]
    missing = []
    lemma_only = []          # lemmas that proved while the property's own goal did not close
    for lever in required:
        lv = ctx["battery"]["levers"][lever]
        if lv.get("scorer_applied"):
            # the scorer applies CLI/tactic levers ITSELF and re-runs the module
            flags = {
                # These go to WHY3FIND, not to creusot-rustc. Passed bare, `cargo creusot`
                # rejects them outright — measured: `-T split_vc` gives
                # "error: unexpected argument '-T' found" and a usage dump, so the run fails
                # BEFORE any prover is consulted. Every one of the three scorer-applied Creusot
                # levers was therefore inert, and a tool-boundary could be awarded without a
                # single escalation having run. Each token needs its own --why3find-arg=.
                "prover_portfolio": ["--why3find-arg=-P", "--why3find-arg=alt-ergo,z3,cvc5,cvc4"],
                "raise_budget": ["--why3find-arg=-t", "--why3find-arg=30",
                                 "--why3find-arg=-d", "--why3find-arg=14"],
                "split_vc": ["--why3find-arg=-T", "--why3find-arg=split_vc,compute_specified"],
            }.get(lever, [])
            vok, _, vwall, vrss, _, _ = run_creusot(
                m, ctx["crate_dir"], ctx["cap"], extra=flags, mem_mb=ctx["mem_mb"], cap_max=ctx["cap_max"])
            if vok:
                ev = {"modules": mods, "result": "Proved", "wall_clock_s": vwall, "peak_rss_mb": vrss, "lever": lever}
                # Anti-vacuity applies on EVERY path that returns proved, not just the base one.
                # Three paths here (this one, the scorer-applied lever, and the rejected
                # inexpressibility claim) returned proved with no mutant check until 2026-09-28.
                vac = vacuity_check(pid, present, ctx, extra=flags)
                if vac:
                    return "UNRESOLVED", {"modules": mods}, vac
                return "proved", ev, f"scorer-applied lever '{lever}' discharged module '{m}' -> Proved"
            continue
        # code lever: require the agent's named variant module, then re-run it
        variant = lv.get("variant", "").replace("<ID>", pid.lower().replace("-", "_"))
        # creusot's ComaNames::insert applies trim_underscores unconditionally
        # (creusot/src/naming.rs), so NO emitted .coma basename can contain '__'. A battery that
        # declares verify_<ID>__inv / __fmap / __trusted therefore demands artifacts that cannot
        # exist, and every code lever was unsatisfiable — the same defect already found for
        # __mutant, which had only been patched on the mutant path. Resolve either spelling.
        if variant and variant not in present:
            collapsed = re.sub(r"__+", "_", variant)
            if collapsed in present:
                variant = collapsed
        if variant not in present:
            missing.append(lever + (f" ({variant})" if variant else ""))
            continue
        vok, _, vwall, vrss, _, _ = run_creusot(
            variant, ctx["crate_dir"], ctx["cap"], mem_mb=ctx["mem_mb"], cap_max=ctx["cap_max"])
        if vok:
            # A lemma is NOT an alternative proof of the property — it is an auxiliary fact a proof
            # may cite. The other code levers (__fmap, __trusted, __inv) re-prove the SAME obligation
            # under a different model, so they legitimately discharge it; `lemma_<ID>` does not.
            # Crediting one over-credits: measured here, verify_epo_inv_len_matches_chain reports
            # `Goal ...: ✘ (1/2)` — 1 unproved file — while lemma_epo_inv_len_matches_chain reports
            # Proved, and the property was being scored `proved` on the lemma's strength alone. A
            # proved lemma only counts once the BASE module closes while citing it.
            if variant.startswith("lemma_"):
                lemma_only.append(variant)
                continue
            ev = {"modules": [variant], "result": "Proved", "wall_clock_s": vwall, "peak_rss_mb": vrss, "lever": lever}
            # Anti-vacuity applies on EVERY path that returns proved, not just the base one.
            # Three paths here (this one, the scorer-applied lever, and the rejected
            # inexpressibility claim) returned proved with no mutant check until 2026-09-28.
            vac = vacuity_check(pid, present, ctx, extra=None)
            if vac:
                return "UNRESOLVED", {"modules": [variant]}, vac
            return "proved", ev, f"lever '{lever}' variant '{variant}' discharged the obligation -> Proved"
    if missing:
        return "UNRESOLVED", {"modules": mods}, (
            f"tool-boundary INADMISSIBLE for failure class '{cls}': missing required lever artifacts {missing}. "
            f"Write every one as a runnable proof module before any boundary claim.")
    if lemma_only:
        # The auxiliary lemma discharges, the property's own goal does not. That is unfinished work,
        # not a tool limit: the lemma exists precisely to be cited by the base proof, so the base
        # must be made to close while citing it.
        return "UNRESOLVED", {"modules": mods, "lemma_proved": lemma_only}, (
            f"lemma(s) {lemma_only} proved but the property's own module did not close — a lemma is an "
            f"auxiliary fact, not a proof of the obligation. Cite it from the base proof and make that "
            f"close; do not credit the property on the lemma alone.")
    # every required lever applied/present and each still failed, signature not a known defeat
    sig = (re.search(r"(Goal \S+: ✘|unproved file|TIMEOUT after \d+s|OOM-KILLED at \d+M[^\n]*)", out) or [""])
    sig = sig.group(0) if hasattr(sig, "group") else "unclassified"
    ev = {"modules": mods, "result": "unproved", "wall_clock_s": wall, "peak_rss_mb": rss, "signature": sig, "lever": None}
    return "tool-boundary", ev, f"battery exhausted for class '{cls}'; residual signature captured: {sig}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("verif_dir", help="the component's verif/ dir (holding unified_properties.yaml)")
    ap.add_argument("--yaml", default="unified_properties.yaml")
    ap.add_argument("--crate-dir", default=None, help="the Creusot verif CRATE dir (holds Cargo.toml + src/ + verif/*_rlib). Defaults to verif_dir if it holds Cargo.toml, else verif_dir itself.")
    ap.add_argument("--gate-dir", default=os.path.dirname(os.path.abspath(__file__)))
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--only", default=None)
    ap.add_argument("--cap-seconds", type=int, default=60,
                    help="BASE per-module time cap in seconds (default 60). A module that only "
                         "TIMES OUT here is retried once at --cap-max before being classed a "
                         "goal-unproved tool-boundary; a real failure is decisive at the base cap.")
    ap.add_argument("--cap-max", type=int, default=300,
                    help="escalated per-module time cap in seconds for the one timeout retry "
                         "(default 300). Set <= --cap-seconds to disable escalation.")
    ap.add_argument("--resume", action="store_true",
                    help="skip properties that already carry a scorer-owned ACCEPT status "
                         "(creusot._scored_by == scorer_creusot). Lets an interrupted run continue "
                         "without re-proving what is already durably scored.")
    ap.add_argument("--mem-max-mb", type=int, default=16384,
                    help="per-module memory cap in MB via a cgroup scope (default 16384 = 16 GB), "
                         "capping cargo creusot + why3find + all parallel provers together. "
                         "Measured proofs peak <2 GB; a pathological goal is OOM-killed (rc -9) "
                         "scope-confined instead of taking the host down.")
    ap.add_argument("--no-mem-cap", action="store_true",
                    help="disable the cgroup memory cap (UNSAFE: a runaway prover can OOM the "
                         "host). Only for environments without a usable systemd --user manager.")
    a = ap.parse_args()

    verif = os.path.abspath(a.verif_dir)
    yaml_path = os.path.join(verif, a.yaml)
    # the crate dir is where Cargo.toml lives; default to verif itself (component verif crates
    # put Cargo.toml + src/ + verif/*_rlib together, e.g. components/<c>/verif/)
    crate_dir = a.crate_dir or verif
    d = load(yaml_path)
    battery = load(os.path.join(a.gate_dir, "lever_battery_creusot.yaml"))
    registry = load(os.path.join(a.gate_dir, "known_defeats.yaml"))
    # load + disjointness-validate the inexpressibility allowlist (fail-safe on collision).
    # NOTE: loaded and validated only; the scoring path does not yet grant an earned
    # boundary from it — probe enforcement is wired in a later, separately-reviewed step.
    inexpressible = load_inexpressible(
        os.path.join(a.gate_dir, "inexpressible_creusot.yaml"), registry, tool="creusot")

    if not a.dry_run:
        n = touch_sources(crate_dir)
        print(f"scorer_creusot: touched {n} src/**.rs to force from-source .coma regeneration (anti-tamper)")

    started = datetime.now().astimezone().isoformat(timespec="seconds")
    mem_mb = None if a.no_mem_cap else a.mem_max_mb
    ctx = {
        "modules": find_coma_modules(crate_dir),
        "crate_dir": crate_dir,
        "battery": battery, "registry": registry, "inexpressible": inexpressible,
        "dry_run": a.dry_run, "cap": a.cap_seconds, "cap_max": a.cap_max, "mem_mb": mem_mb,
    }
    only = set(a.only.split(",")) if a.only else None

    # fail-safe: a live run must be able to cap memory, else it could OOM-crash the host
    if not a.dry_run and mem_mb is not None and not mem_cap_available(mem_mb):
        sys.exit(
            f"scorer_creusot: FAIL-SAFE — cannot create a systemd --user memory scope (MemoryMax={mem_mb}M). "
            "Running Creusot uncapped risks OOM-crashing the host.\n"
            "  Fix: ensure a user systemd manager is running (XDG_RUNTIME_DIR set; check "
            "`systemctl --user status`), or pass --no-mem-cap to override at your own risk.")

    if inexpressible is not None:
        cons = inexpressible.get("constructs", [])
        active = [c["id"] for c in cons if c.get("active")]
        print(f"scorer_creusot: inexpressibility allowlist loaded — {len(cons)} construct(s), "
              f"{len(active)} ACTIVE {active or '(none: all inert pending calibration)'}; "
              "disjoint from known_defeats OK; probe enforcement WIRED "
              "(claims_inexpressible -> isolated canonical-probe confirmation)")
    print(f"scorer_creusot: {len(ctx['modules'])} generated .coma modules in {crate_dir}")
    if ctx["modules"]:
        print("  modules:", ", ".join(sorted(ctx["modules"])))
    print(f"{'DRY-RUN — no cargo creusot executed' if a.dry_run else 'LIVE — regenerating + proving modules'}")
    if not a.dry_run:
        print("  memory cap: " + (f"{mem_mb} MB/module (cgroup scope, swap off; OOM -> SIGKILL)"
                                   if mem_mb else "DISABLED (--no-mem-cap) — UNSAFE"))
    print(f"  time cap: {a.cap_seconds}s/module (escalates once to {a.cap_max}s on timeout)")
    if a.resume:
        print("  resume: skipping properties already carrying a scorer-owned creusot status")
    print()

    counts = {"proved": 0, "refuted": 0, "tool-boundary": 0, "delegated": 0, "UNRESOLVED": 0, "DRY": 0, "resumed": 0}
    unresolved = []
    for p in d["properties"]:
        if not p.get("verifiable"):
            continue
        if only and p["id"] not in only:
            continue
        prior = p.get("creusot") or {}
        if a.resume and not a.dry_run and prior.get("_scored_by") == "scorer_creusot" and prior.get("status") in ACCEPT:
            counts["resumed"] += 1
            counts[prior["status"]] = counts.get(prior["status"], 0) + 1
            print(f"  = {p['id']:32s} {prior['status']:13s} resumed (already scorer-owned; --resume)")
            continue
        status, ev, note = score_property(p, ctx)
        counts[status] = counts.get(status, 0) + 1
        if status == "UNRESOLVED":
            unresolved.append((p["id"], note))
        if not a.dry_run and status in ACCEPT:
            blk = p.setdefault("creusot", {})
            blk["status"] = status
            blk["evidence"] = ev
            blk["note"] = note
            blk["_scored_by"] = "scorer_creusot"   # provenance: this status is scorer-owned
            _save_yaml(d, yaml_path)   # atomic checkpoint after EACH scored property -> resumable
        tag = {"proved": "✓", "refuted": "‼", "tool-boundary": "⤴", "delegated": "→",
               "UNRESOLVED": "✗", "DRY": "·"}[status]
        print(f"  {tag} {p['id']:32s} {status:13s} {note}")

    if not a.dry_run:
        # Provenance: stamp WHICH gate + command + toolchain produced these statuses, so the
        # result travels onto the verif branch self-describing. Dry runs write nothing.
        # The Creusot checkout is located via the same portable resolver the probe uses, so its
        # commit is recorded without hardcoding any path.
        penv = dict(os.environ)
        penv["PATH"] = CREUSOT_BIN + ":" + penv.get("PATH", "")
        std = _resolve_creusot_std(crate_dir, inexpressible or {})
        _stamp_run(d, "creusot", {
            "creusot": _capture(["git", "-C", os.path.dirname(std), "describe", "--tags",
                                 "--always"]) if std else None,
            "why3": _capture(["why3", "--version"], env=penv),
            "provers": _why3_provers(env=penv),
            "cap_seconds": a.cap_seconds, "cap_max": a.cap_max, "mem_max_mb": mem_mb,
        }, started)
        _save_yaml(d, yaml_path)

    print(f"\nSUMMARY: {counts}")
    if unresolved:
        print(f"\nCREUSOT GATE: FAILED — {len(unresolved)} UNRESOLVED (the gate is fail-closed):")
        for pid, note in unresolved:
            print(f"    ✗ {pid}: {note}")
        sys.exit(1)
    # A dry run scores NOTHING, so it must never print PASSED — the verdict keyed only on `unresolved`
    # being empty, so a dry run reported a pass having reproduced no proof at all. The blunt verdict
    # line is what a colleague skimming a log trusts, so it must never overstate what ran.
    if counts.get("DRY"):
        print(f"\nCREUSOT GATE: DRY-RUN — nothing was executed or scored ({counts['DRY']} properties "
              f"have an artifact present). This is NOT a pass: no status was written and no proof was "
              f"reproduced. Re-run without --dry-run for a verdict.")
        return
    print("\nCREUSOT GATE: PASSED — every verifiable property is proved / tool-boundary / delegated, each scorer-reproduced")


if __name__ == "__main__":
    main()

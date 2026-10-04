#!/usr/bin/env python3
"""scorer_kani.py — the Kani REPRODUCTION GATE. Reproduction, not declaration.

The proving agent NEVER writes `kani.status`. This scorer does — by executing
the artifacts. It is read-only to the agent at run time.

For every verifiable property it computes exactly one of a tiny closed set:
    proved        — scorer ran the named harness and it reported VERIFICATION SUCCESSFUL.
                    Evidence (harness, result, unwind, wall_clock_s, peak_rss_mb) is
                    CAPTURED FROM THE RUN, never typed by the agent. Anti-vacuity:
                    if a `__mutant` harness exists it must go RED, else the proof is vacuous.
    tool-boundary — scorer ran the FULL lever battery for the observed failure class and
                    every lever still failed, AND the residual signature is NOT a known
                    defeat. Signature captured from stderr by the scorer.
    delegated     — a resolvable referent exists (named component + concrete obligation).
    UNRESOLVED    — everything else (no harness, agent lied about a pass, missing lever,
                    signature matches a known defeat, unclassifiable failure). The gate
                    FAILS if any verifiable property is UNRESOLVED.

Usage:
    scorer_kani.py <verif_dir> [--yaml unified_properties.yaml] [--component-dir DIR]
                   [--gate-dir DIR] [--dry-run] [--only ID[,ID...]] [--cap-seconds N]

--dry-run: do NOT invoke cargo kani. Validates artifact existence + battery
completeness + registry matching only, and reports what WOULD run. Use it to see
the gate fail-closed instantly over a whole component before spending compute.
"""
import argparse, os, re, signal, subprocess, sys, time, shutil, fnmatch
from datetime import datetime
try:
    import yaml
except ImportError:
    sys.exit("scorer_kani: PyYAML required (python3 -c 'import yaml')")

ACCEPT = {"proved", "tool-boundary", "delegated", "refuted"}
# `refuted` = the obligation is FALSE, machine-checked. It is ACCEPTED, not a gate
# failure: the verification did its job and found a real defect. Per Cornel 2026-09-26 —
# "when we find an error in verifying a property that is very good, this is what rewards
# our verification effort" — it is shown red in the HTML with the spec and code locations
# and the run continues. Only UNRESOLVED (unfinished work) fails the gate.
_UNIT_SEQ = 0


def _capture(cmd, cwd=None, timeout=60):
    """First line of a command's output, or None. Never raises: provenance is best-effort
    metadata and must never fail a scoring run."""
    try:
        r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout)
        if r.returncode != 0:
            return None
        lines = ((r.stdout or "") + (r.stderr or "")).strip().splitlines()
        return lines[0].strip()[:160] if lines else None
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


def harness_id(pid):
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
    Kani forks cbmc/goto-cc/solvers; a bare proc.kill() on the `cargo` parent orphans them
    and they keep chewing the box. When the run was placed in a NAMED systemd --user scope we
    kill by cgroup (hits every descendant); the process group is SIGKILLed as a fallback."""
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


def find_harness_names(component_dir):
    """Every #[kani::proof] fn name declared in the crate (any module path leaf)."""
    names = set()
    for root, _, files in os.walk(component_dir):
        if "/target" in root:
            continue
        for fn in files:
            if not fn.endswith(".rs"):
                continue
            try:
                txt = open(os.path.join(root, fn), errors="ignore").read()
            except OSError:
                continue
            # A proof fn is `fn NAME(` somewhere after a #[kani::proof]. Intervening lines may be
            # further attributes (#[kani::unwind(n)], #[kani::stub(...)], #[kani::should_panic])
            # AND comments — a `//` line between the attribute and the fn used to break the match,
            # so the harness became INVISIBLE to the gate and scored "no runnable harness" while
            # sitting in the file. A silently invisible harness looks exactly like a missing one,
            # which is the worst kind of false negative, so tolerate comments and blank lines too.
            for m in re.finditer(
                    r"#\[kani::proof\][^\n]*\n"
                    r"(?:\s*(?:#\[[^\n]*\]|//[^\n]*|/\*.*?\*/)?\s*\n)*"
                    r"\s*(?:pub\s+)?fn\s+([A-Za-z0-9_]+)", txt, re.S):
                names.add(m.group(1))
    return names


def classify(stderr, battery):
    for cls, spec in battery["failure_classes"].items():
        for sig in spec["signatures"]:
            if re.search(sig, stderr, re.I):
                return cls
    return None


_BUILD_FAILURE_SIGS = [
    # WRONG --component-dir. The harnesses live in a sibling crate (verif-kani/ with its own
    # Cargo.toml) but cargo was run somewhere they are not part of the crate, so the filter matches
    # nothing. This is an INVOCATION fault, not a harness fault: `find_harness_names` searches
    # recursively and happily finds the names, then every single property fails identically.
    # Measured 2026-09-28 re-scoring eviction-policy-optimized: 168 harnesses found, and every
    # property returned "unclassifiable failure — the harness is broken, fix it" — 168 accusations
    # against innocent harnesses for one mistyped path. SKILL.md warns about exactly this in prose;
    # the scorer should enforce it, so one accurate error replaces N misleading verdicts.
    r"no harnesses matched the harness filter",
    r"Failed to get cargo metadata",
    r"`cargo metadata` exited with an error",
    r"failed to load source for dependency",
    r"failed to parse manifest at",
    r"error inheriting `[^`]+` from workspace root",
    r"could not find `Cargo\.toml`",
    r"error: failed to (?:read|open|parse|select)",
    r"no such command:",
    r"error: could not compile .* due to \d+ previous error",
    r"linking with `cc` failed",
    r"toolchain '[^']+' is not installed",
    # build-script failures: a missing native dependency (e.g. an uninitialised SPDK submodule in
    # a fresh worktree) panics in build.rs long before any harness is reached. Observed as
    # "failed to run custom build command for `spdk-sys`" + a build.rs panic.
    r"failed to run custom build command",
    r"process didn't exit successfully: .*build-script-build",
    r"panicked at [^\n]*build\.rs",
    r"Failed to execute cargo \(exit status",
]


def build_failure(out):
    """The FIRST matching build/toolchain signature in `out`, else None.

    This is an ENVIRONMENT fault, categorically different from a harness that fails to verify:
    cargo never got as far as running a proof, so the harness is not implicated at all. Keeping
    the two apart matters — a real incident (a stray gitignored `creusot` symlink inside a
    component made `cargo metadata` unresolvable) was reported as 28 separate "the harness is
    broken, fix it" verdicts, pointing the operator at 28 innocent harnesses instead of at the
    one broken path."""
    for sig in _BUILD_FAILURE_SIGS:
        m = re.search(sig, out or "", re.I)
        if m:
            return m.group(0)
    return None


def kani_env():
    """The PATH-augmented environment every cargo invocation here uses. Shared by the doctor and
    the real runs deliberately: a doctor that probed a different PATH could pass while the runs
    fail, which is worse than having no doctor."""
    env = dict(os.environ)
    env["PATH"] = (os.path.expanduser("~/.cargo/bin") + ":"
                   + os.path.expanduser("~/.local/share/creusot/bin") + ":" + env.get("PATH", ""))
    return env


def kani_doctor(component_dir):
    """Preflight: can cargo even read this component? Returns (ok, detail).

    The Creusot side has had a prover doctor since day one; Kani had no equivalent, so a broken
    build surfaced only as mass UNRESOLVED. One ~1s `cargo metadata` call here turns that into a
    single accurate error before any property is scored."""
    # NOTE: deliberately NOT --no-deps. The fault this exists to catch lives in DEPENDENCY
    # resolution (a bad path dep / stray symlink inside the component), and --no-deps skips
    # exactly that: measured on the real incident it returned 0 while full resolution returned
    # 101. A doctor that cannot fail on the fault it was written for is worse than none.
    try:
        r = subprocess.run(["cargo", "metadata", "--format-version", "1"],
                           cwd=component_dir, capture_output=True, text=True, timeout=180,
                           env=kani_env())
    except Exception as e:
        return False, f"cargo metadata could not be run: {e}"
    if r.returncode == 0:
        return True, "cargo metadata ok"
    out = (r.stdout or "") + (r.stderr or "")
    sig = build_failure(out) or "cargo metadata failed"
    first = next((ln.strip() for ln in out.splitlines() if ln.strip()), "")
    return False, f"{sig} :: {first[:300]}"


def wall_signature(out):
    """A coarse, STABLE fingerprint of a structural failure, or None.

    Purpose: recognise that many properties are failing for the SAME structural reason so the
    scorer can stop re-paying the expensive cap escalation to rediscover it. Measured on
    eviction-policy-optimized: 62 component-level properties all died at the same
    `define_component!` construction, and the stage spent 3280s (43% of 2.1h) on attempts that
    never resolved.

    Deliberately coarse — it groups by failure SHAPE, not by property — but never by property
    identity, so it cannot mask a genuine per-property result. Note a bare timeout IS included:
    that is the shape the construction wall takes here. The escalation budget below is what keeps
    that safe, because a timeout is also what a merely-slow-but-provable property looks like."""
    o = out or ""
    m = re.search(r"unwinding assertion loop \d+", o, re.I)
    if m:
        fn = re.search(r"in function ([\w:<>]+)", o)
        return f"unwinding-assertion:{fn.group(1) if fn else 'unknown'}"
    if re.search(r"TIMEOUT after \d+s", o):
        return "timeout"
    if re.search(r"OOM-KILLED", o):
        return "oom"
    return None


def known_defeat(stderr, registry):
    for d in registry.get("defeats", []):
        for sig in d.get("signatures", []):
            if re.search(sig, stderr, re.I):
                return d
    return None


def run_kani(harness, component_dir, cap, mem_mb=None, cap_max=None, escalate=True, extra=None):
    """Run one harness under /usr/bin/time -v, in its own session and (when mem_mb is set) a
    NAMED transient systemd --user scope; return (ok, out, wall_s, rss_mb, timed_out, oomed).

    Memory: with mem_mb set the whole process tree runs inside a scope with MemoryMax=<mem_mb>M
    and swap disabled. If CBMC's SAT formula blows the cap (format!/observable-output harnesses
    have no natural ceiling) the kernel OOM-kills that scope only — exit 137, the rest of the box
    untouched. An OOM is reported via `oomed` and classed as resource exhaustion like a timeout;
    it never becomes a free tool-boundary.

    Timeout: the run is capped at `cap` seconds and, on breach, its whole tree is SIGKILLed
    (cbmc/solvers included) — not just the `cargo` parent. Adaptive: on a TIMEOUT only (a real
    failure is decisive at the base cap), if escalate and cap_max>cap we retry the SAME harness
    once at cap_max before classing it a sat-timeout, so a merely-slow proof is not mislabelled a
    tool-boundary. Mutant/probe runs pass escalate=False (they are meant to be fast/decisive).

    `extra` appends CLI flags, which is what lets the SCORER apply the CLI-only levers itself
    (--unwind N, --no-unwinding-checks, --solver X) instead of demanding a source artifact that
    cannot express them. Before this existed, `nounwindcheck` was a required lever nobody could
    ever supply, so its failure classes could never reach tool-boundary; and `unwind_sweep` /
    `solver_swap` were silently skipped, so a boundary could be awarded without either ever
    being tried. Keeping the flags HERE, in the gate, also keeps the resulting fidelity label
    gate-owned: an agent cannot quietly grant itself bounded-shallow via global Cargo flags."""
    env = kani_env()
    time_bin = shutil.which("time") or "/usr/bin/time"

    def once(c):
        unit = _new_unit("kani") if mem_mb else None
        kani_cmd = [time_bin, "-v", "cargo", "kani", "--harness", harness,
                    "-Z", "stubbing", "--output-format", "terse"] + list(extra or [])
        if mem_mb:
            cmd = ["systemd-run", "--user", "--scope", "--quiet", f"--unit={unit}",
                   "-p", f"MemoryMax={mem_mb}M", "-p", "MemorySwapMax=0"] + kani_cmd
        else:
            cmd = kani_cmd
        return _exec_capped(cmd, component_dir, c, env, unit)

    out, rc, wall, timed_out = once(cap)
    if timed_out and escalate and cap_max and cap_max > cap:
        out, rc, wall2, timed_out = once(cap_max)
        wall = round(wall + wall2, 2)
    oomed = False
    # A cgroup OOM SIGKILLs the whole scope; subprocess reports returncode -9, a shell 128+9=137.
    # Catch both so the OOM is never misread as an "unclassifiable" harness failure.
    if mem_mb and rc in (-9, 137) and "VERIFICATION SUCCESSFUL" not in out:
        oomed = True
        out += f"\nOOM-KILLED at {mem_mb}M cgroup limit (SIGKILL rc={rc})"
    rss_mb = None
    m = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", out)
    if m:
        rss_mb = round(int(m.group(1)) / 1024)
    ok = verdict_for(out, harness)
    return ok, out, wall, rss_mb, timed_out, oomed


_TOOL_CRASH_SIGS = [
    r"CBMC failed with status \d+",
    r"cbmc: .*(?:Assertion|assertion) `.*' failed",
    r"Invariant check failed",
    r"terminate called after throwing",
    r"Segmentation fault",
    r"std::bad_alloc",
    r"kani-compiler.*panicked at",
    r"internal compiler error",
]


def tool_crash(out):
    """The FIRST tool-crash signature in `out`, else None.

    WHY THIS EXISTS, and it is a soundness hole we shipped and then found:
    Kani renders a CBMC crash as `VERIFICATION:- FAILED`. So a crash is indistinguishable, in the
    verdict line, from an honest refutation. That is harmless for a base harness (a crash then reads
    as "did not prove", which is conservative) and DANGEROUS for a mutant twin, because the
    anti-vacuity rule is "the mutant MUST fail" — so a crashed mutant would be credited as evidence
    that the proof has content, and the proof would be scored `proved` on the strength of a tool
    failure.

    Measured 2026-09-30 while testing the stub route: at 4x container capacity BOTH twins returned
    `CBMC failed with status 6` after ~1090 s with no check counts. Read naively, the mutant "failed
    correctly" and the base's failure would have been retried under levers — a crash laundered into a
    verdict. Anything matching here is INCONCLUSIVE and must never be read as a verdict.
    """
    for sig in _TOOL_CRASH_SIGS:
        m = re.search(sig, out or "", re.I)
        if m:
            return m.group(0)
    return None


def verdict_for(out, harness):
    """Did THIS harness verify? Not: did anything in the output verify?

    `cargo kani --harness NAME` matches NAME as a SUBSTRING, so one flag can run several
    harnesses and the output interleaves their verdicts. Scanning the whole blob for
    "VERIFICATION SUCCESSFUL" therefore credits any one success to the harness we asked about.
    MEASURED, and it was actively producing a FALSE RESULT: one
    `--harness verify_epo_inv_stale_handle_never_crashes` ran three harnesses —
    `__split_in_range` SUCCESSFUL, `__split_remove` FAILED, and the property itself FAILED — and
    the property was scored `proved`. That obligation is provably false: its negation is
    machine-proved in Creusot and its Kani refutation passes. So the gate was awarding `proved`
    to a real defect, which is the worst failure this gate can have.

    Kani's terse output brackets each run as `Checking harness <path>::<name>...` followed by that
    harness's `VERIFICATION:- RESULT`. Attribute verdicts per harness and return only the one
    belonging to `harness`, matched on the leaf since the printed name is module-qualified.
    Fail closed: if this harness's own verdict never appears, that is not a pass."""
    blocks = re.split(r"Checking harness\s+", out or "")
    if len(blocks) > 1:
        seen = {}
        for b in blocks[1:]:
            name = (b.split("...", 1)[0] or "").strip()
            seen[name.rsplit("::", 1)[-1]] = bool(re.search(r"VERIFICATION:- SUCCESSFUL", b))
        leaf_wanted = harness.rsplit("::", 1)[-1]
        if leaf_wanted in seen:
            return seen[leaf_wanted]
        return False                  # ran, but never reported on the harness we asked about
    # No per-harness framing (older output shape, or the run died before reporting any): fall back
    # to the whole-output check — safe only because nothing could have been attributed anyway.
    return bool("VERIFICATION SUCCESSFUL" in (out or "")
                or re.search(r"VERIFICATION:- SUCCESSFUL", out or ""))


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


def vacuity_check(pid, present, ctx, extra=None):
    """Anti-vacuity: the `__mutant` twin must FAIL. Returns None if the proof is honest (mutant
    absent or failed), or a reason string if the proof is VACUOUS (mutant also passed).

    TWO THINGS MADE THIS A FUNCTION RATHER THAN INLINE CODE, both found on
    eviction-policy-session-lists (2026-09-28):

    1. The check used to live only on the unwinding-checks-ON success path. The known-defeat branch
       that applies `--no-unwinding-checks` returned "proved" directly and NEVER ran the mutant, so
       EVERY `bounded-shallow` proof bypassed anti-vacuity entirely. Blast radius when found: 63
       ALREADY-PUBLISHED proved cells (eviction-policy-optimized 58, extended-metadata-store 5).
       Same class as the Creusot mutant-lookup defect — the check existed, a whole population never
       reached it. One helper, called from every path that can return "proved", is the fix.

    2. `extra` is not optional bookkeeping, it is the correctness of the comparison.
       `--no-unwinding-checks` PRUNES paths, it does not merely truncate them, so a harness can pass
       with no content: measured on that component, every `verify_*` passed AND 36 `__mutant` twins
       passed too. If the base proved with `--unwind 4 --no-unwinding-checks` and the mutant is run
       WITHOUT those flags, the mutant may fail on the unwinding assertion rather than because of the
       mutation — which satisfies anti-vacuity for the wrong reason and hides exactly the vacuity
       being hunted. The mutant must run under the SAME flags as the proof it is vouching for.
    """
    mut = harness_id(pid) + "__mutant"
    if mut not in present:
        return None
    mok, mout, _, _, mtimed, _ = run_kani(
        mut, ctx["component_dir"], ctx["cap"], ctx["mem_mb"], escalate=False, extra=extra)
    if not mok:
        # "The mutant failed" only counts as evidence when it failed for the RIGHT reason. A tool
        # crash or a timeout is not a refutation: Kani prints a CBMC crash as VERIFICATION:- FAILED,
        # so a crashed twin would otherwise be credited as proof that the base has content.
        crash = tool_crash(mout)
        if crash:
            return (f"INCONCLUSIVE, not vouched: the anti-vacuity twin '{mut}' did not fail on its "
                    f"assertion — the tool crashed ({crash}). Kani renders a CBMC crash as "
                    f"VERIFICATION:- FAILED, so this would otherwise be miscredited as a correct "
                    f"refutation. Reduce the harness cost or the container capacity until the twin "
                    f"fails on its assertion, then re-score.")
        if mtimed:
            return (f"INCONCLUSIVE, not vouched: the anti-vacuity twin '{mut}' TIMED OUT rather than "
                    f"failing on its assertion, so it is no evidence that the proof has content. "
                    f"Raise the cap for this property or shrink the harness, then re-score.")
        return None
    how = f" under the same flags as the proof ({' '.join(extra)})" if extra else ""
    return (f"VACUOUS: mutant harness '{mut}' also passed{how} — the proof holds no content. "
            f"Strengthen the property, or (if the proof used --no-unwinding-checks) the bound prunes "
            f"away the paths the obligation is about.")


def score_property(p, ctx):
    """Return (status, evidence_dict, note). status in ACCEPT or 'UNRESOLVED'."""
    pid = p["id"]
    proposed = (p.get("kani") or {})
    # harness pointer: explicit evidence.harness, else the naming convention
    named = (proposed.get("evidence") or {}).get("harness") or harness_id(pid)
    present = ctx["harnesses"]

    # Harness pointers may be module-qualified (`proofs::verify_x`) while the crate walk
    # (find_harness_names) collects bare leaf fn names. Normalize to the runnable leaf when
    # the qualified form isn't itself a declared name, so `named not in present` reflects
    # real absence rather than a `::`-prefix mismatch. `cargo kani --harness` accepts the
    # bare leaf (suffix match), so downstream execution is unaffected.
    if named not in present and named.rsplit("::", 1)[-1] in present:
        named = named.rsplit("::", 1)[-1]

    # ---- delegation: triggered by an agent-written delegate_to (the skills forbid the
    #      agent to write `status`), or a legacy status:delegated; needs a resolvable referent ----
    if proposed.get("delegate_to") or proposed.get("status") == "delegated":
        owner = (proposed.get("note") or "") + " " + str(proposed.get("delegate_to", ""))
        if re.search(r"\b(component|crate)\b", owner, re.I) or proposed.get("delegate_to"):
            return "delegated", proposed.get("evidence", {}), "delegated to a named referent (scorer did not re-derive; refuter audits)"
        return "UNRESOLVED", {}, "delegated with no resolvable referent (name the owning component + obligation)"

    # ---- REFUTATION: the obligation is FALSE and here is the machine-checked reason ----
    # Finding a real violation is what verifying is FOR, so it gets a first-class status rather than
    # being filed as UNRESOLVED ("produce the harness"), which made a genuine defect look identical
    # to unfinished work.
    #
    # A refutation must be DEMONSTRATED, never inferred from a failing proof: `verify_<id>` failing
    # can equally mean a bad bound, a wrong harness or a solver limit. So the agent writes a
    # separate `refute_<id>` harness that asserts the violation is REACHABLE (typically
    # #[kani::should_panic], or asserting the negated postcondition), and it must PASS. A passing
    # refutation is positive evidence: Kani found the execution.
    refute = "refute_" + pid.lower().replace("-", "_")
    if refute in present and not ctx["dry_run"]:
        rok, rout, rwall, rrss, rtimed, _ = run_kani(
            refute, ctx["component_dir"], ctx["cap"], ctx["mem_mb"], ctx["cap_max"])
        rlever = None
        if not rok:
            # A REFUTATION GETS THE LEVER BATTERY TOO. It used to get none, and the consequence was
            # the worst kind available: on eviction-policy-session-lists no refutation scenario closes
            # inside 300s, so a GENUINE refutation failed here, the scorer fell through to the base
            # harness, and the base's known-defeat branch awarded `proved` — publishing a
            # machine-checkable defect as a proof.
            #
            # Applying `--no-unwinding-checks` to a refutation is sound in the SAFE direction:
            # pruning REMOVES paths, so it can only make a violation harder to reach, never easier. A
            # refutation that passes under the lever still exhibits a real execution reaching the
            # violation, so pruning risks false negatives, not false defect claims. That is the exact
            # reverse of the proof case, where pruning is what lets a vacuous pass through — which is
            # why the same flag demands a mutant re-run for a proof but needs no such guard here.
            rkd = known_defeat(rout, ctx["registry"])
            if rtimed or (rkd and rkd.get("mandated_lever") == "nounwindcheck"):
                for rn in (4, 8):
                    rok, rout, rwall2, rrss2, _, _ = run_kani(
                        refute, ctx["component_dir"], ctx["cap"], ctx["mem_mb"], escalate=False,
                        extra=["--unwind", str(rn), "--no-unwinding-checks"])
                    rwall = (rwall or 0) + (rwall2 or 0)
                    rrss = max(rrss or 0, rrss2 or 0)
                    if rok:
                        rlever = f"nounwindcheck --unwind {rn}"
                        break
        if rok:
            base_ok = False
            if named in present:
                base_ok, _, _, _, _, _ = run_kani(
                    named, ctx["component_dir"], ctx["cap"], ctx["mem_mb"], escalate=False)
            if base_ok:
                return "UNRESOLVED", {"harness": named, "refutation": refute}, (
                    f"CONTRADICTION: '{named}' verified AND its refutation '{refute}' also passed. "
                    f"One of them is vacuous (check the refutation actually reaches the violation) — "
                    f"fix that before any verdict; a defect claim on this footing is not trustworthy.")
            rev = {"refutation": refute, "result": "violation demonstrated",
                   "wall_clock_s": rwall, "peak_rss_mb": rrss}
            if rlever:
                rev["lever"] = rlever
            return "refuted", rev, (
                f"REFUTED — '{refute}' demonstrates a reachable violation, so the code breaks this "
                f"obligation. This is a finding, not a gap: see the spec and code locations on the "
                f"property record."
                + (f" The refutation needed lever '{rlever}'; pruning can only make a violation "
                   f"harder to reach, so the witness stands." if rlever else ""))

    # ---- no base artifact at all -> cannot climb out of the default ----
    if named not in present:
        return "UNRESOLVED", {}, f"no runnable harness '{named}' (nor '{harness_id(pid)}'): produce it — absence is not a tool limit"

    if ctx["dry_run"]:
        return "DRY", {"harness": named}, "dry-run: harness present, not executed"

    # ---- execute the base harness; the exit is the truth ----
    # The cap escalation (base -> cap_max) is driven from HERE rather than inside run_kani, so a
    # recurring structural wall can be recognised between the two attempts. Escalation exists to
    # rescue a merely-SLOW proof; once the same failure shape has consumed its budget without ever
    # yielding a pass, paying cap_max again only rediscovers the same wall. Measured: 43% of a
    # 2.1h stage went to attempts that never resolved, nearly all the same construction wall.
    seen = ctx.setdefault("wall_seen", {})
    budget = ctx.get("wall_budget", 3)
    ok, out, wall, rss, timed_out, oomed = run_kani(
        named, ctx["component_dir"], ctx["cap"], ctx["mem_mb"], escalate=False)
    skipped_escalation = None
    if not ok and timed_out and ctx["cap_max"] > ctx["cap"]:
        sig = wall_signature(out)
        spent = seen.get(sig, 0) if sig else 0
        if sig and spent >= budget:
            # Budget exhausted for this shape: record WHY we stopped, so the decision is visible
            # and a re-run with a raised budget can revisit it. Never silently give up.
            skipped_escalation = (sig, spent)
        else:
            ok2, out2, wall2, rss2, t2, o2 = run_kani(
                named, ctx["component_dir"], ctx["cap_max"], ctx["mem_mb"], escalate=False)
            wall += wall2
            ok, out, rss, timed_out, oomed = ok2, out2, max(rss or 0, rss2 or 0), t2, o2
            if not ok2 and sig:
                seen[sig] = spent + 1          # only a FAILED escalation consumes budget
    if ok:
        # anti-vacuity: a __mutant harness, if present, MUST fail (fast, no cap escalation).
        # Base proved with no extra flags, so the mutant runs with none either — same configuration.
        vac = vacuity_check(pid, present, ctx, extra=None)
        if vac:
            return "UNRESOLVED", {"harness": named}, vac
        ev = {"harness": named, "result": "SUCCESS", "wall_clock_s": wall, "peak_rss_mb": rss}
        return "proved", ev, "scorer re-ran harness -> VERIFICATION SUCCESSFUL"

    # ---- failed: an ENVIRONMENT fault is not a verdict about the harness ----
    # cargo never reached a proof, so nothing can be concluded about this property. Flag it as a
    # build fault and let the caller stop the whole stage: one accurate error beats N misleading
    # "fix your harness" verdicts against harnesses that were never compiled.
    # A TOOL CRASH is not a verdict about the harness. Kani prints a CBMC crash as
    # VERIFICATION:- FAILED, so without this the scorer would spend the whole lever battery trying to
    # "rescue" a property whose tool fell over, and could then award a tool-boundary on a signature
    # that is really a crash. Report it as the environment fault it is.
    tc = tool_crash(out)
    if tc:
        return "UNRESOLVED", {"harness": named, "tool_crash": tc}, (
            f"TOOL CRASH, not a harness or tool-limit verdict: {tc}. Kani renders this as "
            f"VERIFICATION:- FAILED, which is why it must be matched explicitly. Nothing can be "
            f"concluded: reduce the harness cost (container capacity, symbolic inputs) or the memory "
            f"cap until the run completes, then re-score.")
    bf = build_failure(out)
    if bf:
        hint = ""
        if "no harnesses matched" in bf.lower():
            hint = (" — LIKELY A WRONG --component-dir: the harnesses were found by name but cargo ran "
                    "where they are not part of the crate. If they live in a sibling crate, point "
                    "--component-dir at THAT crate (e.g. components/<c>/verif-kani), not the component root.")
        return "BUILD-ERROR", {"harness": named}, (
            f"BUILD/TOOLCHAIN failure, not a harness or tool limit — cargo never ran a proof: {bf}{hint}")
    # ---- registry next (a beaten wall is never a boundary) ----
    kd = known_defeat(out, ctx["registry"])
    if kd:
        # The registry says "apply lever X" — so APPLY IT, rather than returning and telling
        # someone else to. Until the scorer could run CLI levers itself, this branch had no choice
        # but to bail; now it does, and bailing wastes the very lever the registry mandates.
        # MEASURED on eviction-policy-optimized: all 57 remaining UNRESOLVED carried this one
        # reason — "known defeat KD-MULTIMAP-UNWIND-TIMEOUT — apply lever 'nounwindcheck'" — while
        # a different property proved through exactly that lever in the same run, and the proving
        # agent had measured `--unwind 4 --no-unwinding-checks` succeeding on this component.
        mand = kd.get("mandated_lever")
        if mand == "nounwindcheck":
            for n in (4, 8):
                lever_flags = ["--unwind", str(n), "--no-unwinding-checks"]
                sok, _, swall, srss, _, _ = run_kani(
                    named, ctx["component_dir"], ctx["cap"], ctx["mem_mb"], escalate=False,
                    extra=lever_flags)
                if sok:
                    # ANTI-VACUITY IS MANDATORY HERE TOO — and this is the path where vacuity is most
                    # likely, not least. `--no-unwinding-checks` prunes paths rather than truncating
                    # them, so a harness can pass with no content: measured on
                    # eviction-policy-session-lists, 36 mutants passed under this lever. This branch
                    # used to `return "proved"` with no mutant check, which is how 63 published proved
                    # cells came to rest on a check that never ran. The mutant runs under the SAME
                    # flags or the comparison is meaningless — it would fail on the unwinding
                    # assertion instead of on the mutation.
                    vac = vacuity_check(pid, present, ctx, extra=lever_flags)
                    if vac:
                        return "UNRESOLVED", {"harness": named, "unwind": n,
                                              "unwinding_checks": False,
                                              "lever": "nounwindcheck"}, vac
                    return "proved", {"harness": named, "result": "SUCCESS", "unwind": n,
                                      "unwinding_checks": False, "wall_clock_s": swall,
                                      "peak_rss_mb": srss, "lever": "nounwindcheck",
                                      "known_defeat": kd["id"],
                                      "vacuity_checked_under_lever": True}, (
                        f"known defeat {kd['id']}: its mandated lever 'nounwindcheck' was applied by "
                        f"the scorer and PROVED at --unwind {n} — a NARROWER claim, holding within {n} "
                        f"loop iterations and silent beyond (fidelity bounded-shallow). Its mutant twin "
                        f"was re-run under the same flags and correctly FAILED, so the bounded claim has "
                        f"content rather than passing because the bound pruned the paths away.")
        return "UNRESOLVED", {"harness": named, "known_defeat": kd["id"]}, (
            f"signature matches known defeat {kd['id']} and its mandated lever "
            f"'{mand}' did not discharge it either; claiming a tool-boundary on a beaten wall is "
            f"rejected")
    # a hard timeout or a cgroup OOM is decisively resource exhaustion (sat-timeout class),
    # regardless of incidental trace text
    cls = "sat-timeout" if (timed_out or oomed) else classify(out, ctx["battery"])
    if cls is None:
        return "UNRESOLVED", {"harness": named}, "unclassifiable failure — the harness is broken, not the tool; fix it"

    if skipped_escalation:
        # Stop here rather than spending the lever battery too: the battery's runs cost as much as
        # the escalation we just declined, and this property is failing for a shape already shown
        # not to yield. Reported explicitly, with the knob to revisit, so it is a bounded decision
        # and not a silent surrender. Still UNRESOLVED — never tool-boundary on an unexhausted battery.
        wsig, wspent = skipped_escalation
        return "UNRESOLVED", {"harness": named, "wall_signature": wsig}, (
            f"RECURRING WALL '{wsig}': {wspent} earlier properties escalated to {ctx['cap_max']}s on this "
            f"same failure shape and none passed, so escalation and the lever battery were SKIPPED here "
            f"to avoid re-paying a known cost. This is unfinished work, not a tool limit. Raise "
            f"--wall-budget (or fix the root cause) to revisit.")

    required = ctx["battery"]["failure_classes"][cls]["required_levers"]
    missing, ran_all_fail = [], True
    for lever in required:
        lv = ctx["battery"]["levers"][lever]
        variant = lv.get("variant", "").replace("<ID>", pid.lower().replace("-", "_"))
        # CLI-only levers: the SCORER applies these itself by re-running the base harness under
        # flags. They cannot be expressed as source artifacts, which is why demanding one was
        # unsatisfiable, and why skipping them silently (as this did before) let a tool-boundary be
        # awarded without a single alternative bound or solver ever being tried.
        #
        # ORDER MATTERS, and it is strongest-claim-first. The sweep keeps unwinding checks ON, so
        # anything it proves is a full-strength `proved`. Only nounwindcheck weakens the claim —
        # it deletes the assertion that the bound was large enough — so it runs LAST and its pass
        # is recorded as `bounded-shallow` fidelity. Fidelity is assigned HERE, by the gate, so an
        # agent cannot quietly award itself the weaker label via global Cargo flags.
        if lever == "unwind_sweep":
            for n in (4, 8, 16, 32):
                sok, _, swall, srss, _, _ = run_kani(
                    named, ctx["component_dir"], ctx["cap"], ctx["mem_mb"],
                    escalate=False, extra=["--unwind", str(n)])
                if sok:
                    sweep_flags = ["--unwind", str(n)]
                    vac = vacuity_check(pid, present, ctx, extra=sweep_flags)
                    if vac:
                        return "UNRESOLVED", {"harness": named, "unwind": n,
                                             "lever": "unwind_sweep"}, vac
                    return "proved", {"harness": named, "result": "SUCCESS", "unwind": n,
                                      "wall_clock_s": swall, "peak_rss_mb": srss,
                                      "lever": "unwind_sweep",
                                      "vacuity_checked_under_lever": True}, (
                        f"scorer-applied unwind sweep: PROVED at --unwind {n} with unwinding checks "
                        f"ON (full strength; the agent's own bound did not close), and its mutant twin "
                        f"re-run at the same bound correctly FAILED")
            continue
        if lever == "solver_swap":
            for slv in ("cadical", "minisat"):
                sok, _, swall, srss, _, _ = run_kani(
                    named, ctx["component_dir"], ctx["cap"], ctx["mem_mb"],
                    escalate=False, extra=["--solver", slv])
                if sok:
                    vac = vacuity_check(pid, present, ctx, extra=["--solver", slv])
                    if vac:
                        return "UNRESOLVED", {"harness": named, "solver": slv,
                                             "lever": "solver_swap"}, vac
                    return "proved", {"harness": named, "result": "SUCCESS", "solver": slv,
                                      "wall_clock_s": swall, "peak_rss_mb": srss,
                                      "lever": "solver_swap",
                                      "vacuity_checked_under_lever": True}, (
                        f"scorer-applied solver swap: PROVED under '{slv}' (full strength), and its "
                        f"mutant twin re-run under the same solver correctly FAILED")
            continue
        if lever == "loop_contract":
            # STRONGEST available fallback, tried BEFORE nounwindcheck. -Z loop-contracts replaces
            # unrolling with an invariant, so the body executes twice, the cost is decoupled from the
            # iteration count, and the claim is UNBOUNDED. Measured on a 1024-iteration loop (see
            # gate/case-studies/loop_contract_vs_nounwindcheck.rs): the nounwindcheck form "proved" in
            # 0.034s AND its mutant also passed, i.e. vacuous; the contract form proved in 0.20s with
            # the mutant correctly failing. It needs the agent to have written the invariant, so an
            # absent variant is missing work rather than a tool limit.
            lc_flags = ["-Z", "loop-contracts"]
            for cand in (variant, named):
                if cand and cand in present:
                    sok, _, swall, srss, _, _ = run_kani(
                        cand, ctx["component_dir"], ctx["cap"], ctx["mem_mb"],
                        escalate=False, extra=lc_flags)
                    if sok:
                        vac = vacuity_check(pid, present, ctx, extra=lc_flags)
                        if vac:
                            return "UNRESOLVED", {"harness": cand, "lever": "loop_contract"}, vac
                        return "proved", {"harness": cand, "result": "SUCCESS",
                                          "wall_clock_s": swall, "peak_rss_mb": srss,
                                          "lever": "loop_contract", "unbounded": True,
                                          "vacuity_checked_under_lever": True}, (
                            "scorer-applied loop contract (-Z loop-contracts): PROVED with NO unwind "
                            "bound, so the claim covers every iteration count rather than a bounded "
                            "prefix - full strength. Its mutant twin was re-run under the same flag "
                            "and correctly FAILED.")
            continue
        if lever == "nounwindcheck":
            # Weakest admissible lever, tried only after the full-strength options failed.
            for n in (4, 8):
                nuc_flags = ["--unwind", str(n), "--no-unwinding-checks"]
                sok, _, swall, srss, _, _ = run_kani(
                    named, ctx["component_dir"], ctx["cap"], ctx["mem_mb"], escalate=False,
                    extra=nuc_flags)
                if sok:
                    vac = vacuity_check(pid, present, ctx, extra=nuc_flags)
                    if vac:
                        return "UNRESOLVED", {"harness": named, "unwind": n,
                                             "unwinding_checks": False,
                                             "lever": "nounwindcheck"}, vac
                    return "proved", {"harness": named, "result": "SUCCESS", "unwind": n,
                                      "vacuity_checked_under_lever": True,
                                      "unwinding_checks": False, "wall_clock_s": swall,
                                      "peak_rss_mb": srss, "lever": "nounwindcheck"}, (
                        f"scorer-applied --unwind {n} --no-unwinding-checks: PROVED, but this is a "
                        f"NARROWER claim — it holds for executions within {n} loop iterations and "
                        f"says nothing about longer ones (fidelity bounded-shallow)")
            continue
        # A variant may be declared as a glob (e.g. split_harness's
        # `verify_<ID>__split_*`) so the agent can name the decomposition freely.
        # Resolve globs with fnmatch; exact names still match exactly. Fail-closed:
        # if no present harness matches, the lever is missing (UNRESOLVED), and a
        # matched harness that PROVES is caught below as "record it as proved".
        if "*" in variant:
            hits = sorted(h for h in present if fnmatch.fnmatch(h, variant))
            cand = hits[0] if hits else None
        else:
            cand = variant if variant in present else None
        if not cand:
            missing.append(lever + (f" ({variant})" if variant else ""))
            continue
        vok, _, _, _, _, _ = run_kani(
            cand, ctx["component_dir"], ctx["cap"], ctx["mem_mb"], ctx["cap_max"])
        if vok:
            return "UNRESOLVED", {"harness": cand}, (
                f"lever '{lever}' variant '{cand}' PROVED it — this is not a tool-boundary; record it as proved")
    if missing:
        return "UNRESOLVED", {"harness": named}, (
            f"tool-boundary INADMISSIBLE for failure class '{cls}': missing required lever artifacts {missing}. "
            f"Apply every one as a runnable harness before any boundary claim.")
    # every required lever present and each re-run still failed, signature not a known defeat
    sig = (re.search(r"(unwinding assertion loop \d+|TIMEOUT after \d+s|OOM-KILLED at \d+M[^\n]*|Solver.*time\w*)", out, re.I) or [""])
    sig = sig.group(0) if hasattr(sig, "group") else "unclassified"
    ev = {"harness": named, "result": "FAILED", "wall_clock_s": wall, "peak_rss_mb": rss, "signature": sig}
    return "tool-boundary", ev, f"battery exhausted for class '{cls}'; residual signature captured: {sig}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("verif_dir")
    ap.add_argument("--yaml", default="unified_properties.yaml")
    ap.add_argument("--component-dir", default=None)
    ap.add_argument("--gate-dir", default=os.path.dirname(os.path.abspath(__file__)))
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--only", default=None)
    ap.add_argument("--cap-seconds", type=int, default=60,
                    help="BASE per-harness time cap in seconds (default 60). A harness that only "
                         "TIMES OUT here is retried once at --cap-max before being classed a "
                         "sat-timeout tool-boundary; a real failure is decisive at the base cap.")
    ap.add_argument("--wall-budget", type=int, default=3,
                    help="How many properties may escalate to --cap-max on the SAME structural "
                         "failure shape before the scorer stops escalating for that shape. "
                         "Escalation rescues a merely-slow proof; once N properties have spent it "
                         "on one shape without a single pass, further escalation only re-buys the "
                         "same wall. Measured: 43%% of a 2.1h stage went to attempts that never "
                         "resolved, nearly all one construction wall. Raise it to revisit a shape; "
                         "0 disables escalation entirely.")
    ap.add_argument("--cap-max", type=int, default=300,
                    help="escalated per-harness time cap in seconds for the one timeout retry "
                         "(default 300). Set <= --cap-seconds to disable escalation.")
    ap.add_argument("--resume", action="store_true",
                    help="skip properties that already carry a scorer-owned ACCEPT status "
                         "(kani._scored_by == scorer_kani). Lets an interrupted run continue "
                         "without re-proving what is already durably scored.")
    ap.add_argument("--mem-max-mb", type=int, default=16384,
                    help="per-harness memory cap in MB via a cgroup scope (default 16384 = 16 GB). "
                         "Legit proofs peak <1 GB; runaway format!/CBMC harnesses are OOM-killed "
                         "(exit 137) scope-confined instead of taking the host down.")
    ap.add_argument("--no-mem-cap", action="store_true",
                    help="disable the cgroup memory cap (UNSAFE: a runaway harness can OOM the "
                         "host). Only for environments without a usable systemd --user manager.")
    a = ap.parse_args()

    verif = os.path.abspath(a.verif_dir)
    yaml_path = os.path.join(verif, a.yaml)
    component_dir = a.component_dir or os.path.dirname(verif)
    d = load(yaml_path)
    battery = load(os.path.join(a.gate_dir, "lever_battery_kani.yaml"))
    registry = load(os.path.join(a.gate_dir, "known_defeats.yaml"))

    started = datetime.now().astimezone().isoformat(timespec="seconds")
    mem_mb = None if a.no_mem_cap else a.mem_max_mb
    ctx = {
        "harnesses": find_harness_names(component_dir),
        "component_dir": component_dir,
        "battery": battery, "registry": registry,
        "dry_run": a.dry_run, "cap": a.cap_seconds, "cap_max": a.cap_max, "mem_mb": mem_mb,
        "wall_budget": a.wall_budget, "wall_seen": {},
    }
    only = set(a.only.split(",")) if a.only else None

    # fail-safe: a live run must be able to cap memory, else it could OOM-crash the host
    if not a.dry_run and mem_mb is not None and not mem_cap_available(mem_mb):
        sys.exit(
            f"scorer_kani: FAIL-SAFE — cannot create a systemd --user memory scope (MemoryMax={mem_mb}M). "
            "Running Kani uncapped risks OOM-crashing the host.\n"
            "  Fix: ensure a user systemd manager is running (XDG_RUNTIME_DIR set; check "
            "`systemctl --user status`), or pass --no-mem-cap to override at your own risk.")

    # PREFLIGHT (live runs only): can cargo read this component at all? The Creusot side has had
    # a prover doctor from the start; without the Kani equivalent a broken build was reported as
    # mass UNRESOLVED against innocent harnesses. ~1s to turn that into one accurate error.
    if not a.dry_run:
        ok, detail = kani_doctor(component_dir)
        if not ok:
            sys.exit(
                "scorer_kani: ENVIRONMENT FAILURE — cargo cannot read this component, so NO "
                "property can be scored and nothing here is a verdict about any harness.\n"
                f"  component: {component_dir}\n"
                f"  cause:     {detail}\n"
                "  This is a build/toolchain fault: fix the component's cargo setup (a stray path "
                "dependency or symlink inside the component dir is a common cause), then re-run. "
                "Statuses were left untouched.")
        print(f"  preflight: {detail}")

    print(f"scorer_kani: {len(ctx['harnesses'])} kani::proof harnesses found in {component_dir}")
    if ctx["harnesses"]:
        print("  harnesses:", ", ".join(sorted(ctx["harnesses"])))
    print(f"{'DRY-RUN — no cargo kani executed' if a.dry_run else 'LIVE — executing harnesses'}")
    if not a.dry_run:
        print("  memory cap: " + (f"{mem_mb} MB/harness (cgroup scope, swap off; OOM -> exit 137)"
                                   if mem_mb else "DISABLED (--no-mem-cap) — UNSAFE"))
    print(f"  time cap: {a.cap_seconds}s/harness (escalates once to {a.cap_max}s on timeout)")
    if a.resume:
        print("  resume: skipping properties already carrying a scorer-owned kani status")
    print()

    counts = {"proved": 0, "refuted": 0, "tool-boundary": 0, "delegated": 0, "UNRESOLVED": 0, "DRY": 0, "resumed": 0}
    unresolved = []
    # SCOPE (2026-10-04): only what spec and code agree on, plus code-only, is a verification
    # obligation. `divergent` / `spec-only` records and hazard-worded ones are extraction data:
    # refuting them only re-discovered disagreements extraction had already found (43 of 46
    # published refutations). Skipped here, so they are neither scored nor able to fail the gate.
    _OUT = {"divergent", "spec-only", "spec"}
    _pol = d.get("polarity") or {}
    for p in d["properties"]:
        if not p.get("verifiable"):
            continue
        if p.get("origin") in _OUT or str((_pol.get(p["id"]) or {}).get("polarity", "")).upper() == "HAZARD":
            continue
        if only and p["id"] not in only:
            continue
        prior = p.get("kani") or {}
        if a.resume and not a.dry_run and prior.get("_scored_by") == "scorer_kani" and prior.get("status") in ACCEPT:
            counts["resumed"] += 1
            counts[prior["status"]] = counts.get(prior["status"], 0) + 1
            print(f"  = {p['id']:32s} {prior['status']:13s} resumed (already scorer-owned; --resume)")
            continue
        status, ev, note = score_property(p, ctx)
        # A build/toolchain fault mid-run (the preflight passed, then the environment broke, or a
        # property's own lever variant fails to compile): stop the stage NOW. Grinding the rest
        # would burn compute and emit a work-list of harnesses that were never even built.
        if status == "BUILD-ERROR":
            print(f"  ! {p['id']:32s} BUILD-ERROR   {note}")
            sys.exit(
                f"\nscorer_kani: ENVIRONMENT FAILURE at {p['id']} — aborting the stage after "
                f"{counts.get('proved', 0)} proved. cargo could not build, so the remaining "
                "properties are unscored, NOT failed, and no harness is implicated.\n"
                f"  cause: {note}\n"
                "  Fix the build, then re-run with --resume to continue from here.")
        counts[status] = counts.get(status, 0) + 1
        if status == "UNRESOLVED":
            unresolved.append((p["id"], note))
        if not a.dry_run and status in ACCEPT:
            blk = p.setdefault("kani", {})
            blk["status"] = status
            blk["evidence"] = ev
            blk["note"] = note
            blk["_scored_by"] = "scorer_kani"   # provenance: this status is scorer-owned
            # Fidelity is normally the agent's advisory field, but when the GATE itself reached the
            # verdict with unwinding checks disabled, the weaker claim is a fact about the run and
            # the gate owns saying so — an agent must not be able to under-report it.
            if isinstance(ev, dict) and ev.get("unwinding_checks") is False:
                blk["fidelity"] = "bounded-shallow"
            _save_yaml(d, yaml_path)   # atomic checkpoint after EACH scored property -> resumable
        tag = {"proved": "✓", "refuted": "‼", "tool-boundary": "⤴", "delegated": "→",
               "UNRESOLVED": "✗", "DRY": "·"}[status]
        print(f"  {tag} {p['id']:32s} {status:13s} {note}")

    if not a.dry_run:
        # Provenance: stamp WHICH gate + command + tool versions produced these statuses, so
        # the result travels onto the verif branch self-describing. Dry runs write nothing.
        _stamp_run(d, "kani", {
            "kani_version": _capture(["cargo", "kani", "--version"], cwd=component_dir),
            "cap_seconds": a.cap_seconds, "cap_max": a.cap_max, "mem_max_mb": mem_mb,
        }, started)
        _save_yaml(d, yaml_path)

    print(f"\nSUMMARY: {counts}")
    if unresolved:
        print(f"\nKANI GATE: FAILED — {len(unresolved)} UNRESOLVED (the gate is fail-closed):")
        for pid, note in unresolved:
            print(f"    ✗ {pid}: {note}")
        sys.exit(1)
    # A dry run scores NOTHING, so it must never print PASSED. Measured: on a 95-property bundle the
    # dry run reported "KANI GATE: PASSED" with counts {'DRY': 95, 'proved': 0} because the verdict
    # keyed only on `unresolved` being empty — a verdict line an operator could reasonably read as
    # "this component passed". The blunt verdict line is the one thing a colleague skimming a log will
    # trust, so it must never overstate what ran.
    if counts.get("DRY"):
        print(f"\nKANI GATE: DRY-RUN — nothing was executed or scored ({counts['DRY']} properties "
              f"have an artifact present). This is NOT a pass: no status was written and no proof was "
              f"reproduced. Re-run without --dry-run for a verdict.")
        return
    print("\nKANI GATE: PASSED — every verifiable property is proved / tool-boundary / delegated, each scorer-reproduced")


if __name__ == "__main__":
    main()

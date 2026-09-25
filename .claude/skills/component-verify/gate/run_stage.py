#!/usr/bin/env python3
"""run_stage.py — unattended stage-runner for the component-verify pipeline.

Wraps ONE heavy stage (a scorer, a render, any command) with the robustness rails an
unattended, shareable run needs. It does NOT decide verdicts — the wrapped command's own
exit code is propagated; the scorers remain the gate. It only makes running them safe to
leave alone, and legible to a colleague reading the console afterwards:

  * SENTINEL markers on stdout — `START` / `DONE exit=<n>` / `WATCHDOG-KILL` — so a Monitor
    (or a human) can tell unambiguously when the stage began, ended, and with what code.
  * a WATCHDOG wall budget      — if the child outlives it, its WHOLE process tree is
    SIGKILLed (by process group), not just the direct child, and the stage reports 124.
    (Per-harness caps live in the scorers; this is the backstop for the scorer itself
    hanging, e.g. wedged in Python or a pipe.)
  * a readable per-run LOG      — the child's merged stdout+stderr is teed to --log AND to
    this process's stdout, so the full transcript survives for audit.
  * a completion MARKER file    — written with the exit code + duration on ANY outcome, a
    durable signal the orchestrator reads for resumability ("did this stage finish?").

Usage:
  run_stage.py --name kani-scorer --log <c>/verif/.run/kani.log --watchdog 1800 \
               [--marker <c>/verif/.run/.stage_kani.done] -- python3 gate/scorer_kani.py ...
"""
import argparse, os, signal, subprocess, sys, threading, time, datetime


def _iso():
    return datetime.datetime.now().astimezone().isoformat(timespec="seconds")


def _kill_tree(proc):
    """SIGKILL the child's whole process group (it was started in its own session)."""
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except (ProcessLookupError, PermissionError, OSError):
        try:
            proc.kill()
        except Exception:
            pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True, help="stage label used in the SENTINEL lines")
    ap.add_argument("--log", required=True, help="file to tee the child's merged output to")
    ap.add_argument("--watchdog", type=int, default=1800,
                    help="wall-budget seconds; tree-kill the child on breach (default 1800)")
    ap.add_argument("--marker", default=None,
                    help="completion-marker file (default: --log with its extension replaced by .done)")
    ap.add_argument("cmd", nargs=argparse.REMAINDER, help="-- then the command to run")
    a = ap.parse_args()

    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    if not cmd:
        sys.exit("run_stage: no command given (put it after `--`)")

    os.makedirs(os.path.dirname(os.path.abspath(a.log)) or ".", exist_ok=True)
    marker = a.marker or (os.path.splitext(a.log)[0] + ".done")

    t0 = time.time()
    print(f"SENTINEL: {a.name} START {_iso()}", flush=True)
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, bufsize=1, start_new_session=True)

    fired = {"watchdog": False}

    def _bark():
        fired["watchdog"] = True
        print(f"SENTINEL: {a.name} WATCHDOG-KILL after {a.watchdog}s {_iso()}", flush=True)
        _kill_tree(proc)

    timer = threading.Timer(a.watchdog, _bark)
    timer.daemon = True
    timer.start()

    try:
        with open(a.log, "w") as lf:
            lf.write(f"# run_stage {a.name} START {_iso()}\n# cmd: {' '.join(cmd)}\n")
            lf.flush()
            # Iterating proc.stdout blocks until a line arrives OR EOF. EOF arrives both on
            # normal exit and when the watchdog tree-kills the child, so this loop always ends.
            for line in proc.stdout:
                lf.write(line); lf.flush()
                sys.stdout.write(line); sys.stdout.flush()
        proc.wait()
    finally:
        timer.cancel()

    rc = proc.returncode if proc.returncode is not None else 1
    if fired["watchdog"]:
        rc = 124
    code = rc if rc >= 0 else 128 + (-rc)   # signal death -> 128+sig (shell convention)
    code = min(code, 255)
    dur = round(time.time() - t0, 1)

    with open(marker, "w") as mf:
        mf.write(f"stage={a.name}\nexit={code}\nwatchdog_killed={fired['watchdog']}\n"
                 f"seconds={dur}\nfinished={_iso()}\n")

    print(f"SENTINEL: {a.name} DONE exit={code} seconds={dur} {_iso()}", flush=True)
    sys.exit(code)


if __name__ == "__main__":
    main()

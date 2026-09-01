#!/usr/bin/env python3
"""sample_certus_iostat.py — poll certus-server GetIoStats, emit renderer [prom] lines.

The synthetic-agentic Certus-SPDK arm spills evicted KV blocks to an SPDK NVMe
extent tier. Unlike the FS-tier arm, that device bypasses the kernel block layer
and never appears in /proc/diskstats — so sample_diskstats.py sees nothing for it
(scrape returns {}), and the black-box `vllm serve` /metrics the connector exposes
carries only LOGICAL byte movement (kv_offload_*), not physical device I/O. The
result is that render_kvprofile's ssd_read_bytes / ssd_write_bytes bars and
per-round charts stay empty for Certus even though the SSD tier is being hammered.

This poller closes that gap the same way sample_diskstats.py does for the FS-tier
arm, but sources the counters from the certus-server's own GetIoStats shmq op
(dispatcher read/write stats aggregated over all data drives) instead of
/proc/diskstats. It samples on an interval and appends
`[prom] round N: ssd_read_bytes=<delta> ssd_write_bytes=<delta>` lines — the exact
keys render_kvprofile already plots — so no renderer change is needed: the lines
fold into the variant log next to the /metrics scrape rounds, exactly like the
FS-tier disk sidecar. Its round numbers run independently of scrape_prom.py's,
which is fine: render merges tokens by round and sums each key across rounds, and
these two keys are disjoint from the /metrics keys, so the two sidecars fold
together cleanly.

A baseline is taken at startup and NOT emitted, so the summed deltas measure only
the driven window — the counters are cumulative since server boot (they include
any tier warm-up / prior run), and only movement after the baseline is attributed
to this run. GetIoStats reports device-level bytes only (no DRAM-vs-SSD split, no
hit rate at this layer); the tier-movement counts come separately from server.log.

Usage:
  sample_certus_iostat.py --shm-path /dev/shm/certus-shmq --interval 10 --out run.disk.log
"""

from __future__ import annotations

import argparse
import os
import signal
import sys
import time

# The shmq Ring client lives in the certus-shmq-connector package, located via the
# apps/python helper shim (same mechanism as tools/certus-iostat-poll.py). This
# file sits at benchmarks/synthetic-agentic/, so the repo root is three levels up.
# Override the helper location with CERTUS_PY_HELPERS.
_helpers = os.environ.get("CERTUS_PY_HELPERS") or os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    "apps", "python",
)
sys.path.insert(0, _helpers)

_STOP = False


def _handle_stop(signum, _frame):
    global _STOP
    _STOP = True


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--shm-path", default="/dev/shm/certus-shmq",
                    help="certus-server shmq mailbox path")
    ap.add_argument("--interval", type=float, default=10.0, help="seconds between samples")
    ap.add_argument("--out", required=True, help="file to append [prom] lines to")
    args = ap.parse_args()

    try:
        from certus_shmq_helpers import RingError, connect  # noqa: E402
    except ImportError as e:
        print(f"[sample_certus_iostat] cannot import certus_shmq_helpers "
              f"(CERTUS_PY_HELPERS={_helpers!r}): {e}; nothing sampled",
              file=sys.stderr, flush=True)
        return

    signal.signal(signal.SIGTERM, _handle_stop)
    signal.signal(signal.SIGINT, _handle_stop)

    try:
        ring = connect(args.shm_path)
    except (RingError, OSError) as e:
        print(f"[sample_certus_iostat] cannot attach to {args.shm_path!r}: {e}; "
              "nothing sampled", file=sys.stderr, flush=True)
        return
    print(f"[sample_certus_iostat] shm={args.shm_path} op=GetIoStats",
          file=sys.stderr, flush=True)

    def sample() -> dict:
        """{ssd_read_bytes, ssd_write_bytes} cumulative, or {} on a transient error."""
        try:
            r = ring.get_io_stats()
            return {"ssd_read_bytes": float(r["read_bytes"]),
                    "ssd_write_bytes": float(r["write_bytes"])}
        except (RingError, KeyError) as e:  # transient — skip this tick
            print(f"[sample_certus_iostat] get_io_stats failed: {e}",
                  file=sys.stderr, flush=True)
            return {}

    fmt = lambda v: (str(int(v)) if float(v).is_integer() else repr(v))  # noqa: E731

    rnd = 0
    try:
        with open(args.out, "a", encoding="utf-8") as f:
            # Baseline (not emitted): the counters are cumulative since server boot,
            # so subtract the pre-run value and attribute only later movement here.
            prev = sample()
            if not prev:
                print("[sample_certus_iostat] first GetIoStats failed; nothing sampled",
                      file=sys.stderr, flush=True)
                return

            while True:
                waited = 0.0
                while waited < args.interval and not _STOP:
                    time.sleep(0.2)
                    waited += 0.2
                cur = sample()
                if cur:
                    delta = {k: max(0.0, cur.get(k, 0.0) - prev.get(k, 0.0)) for k in cur}
                    shown = " ".join(f"{k}={fmt(delta[k])}" for k in sorted(delta))
                    f.write(f"[prom] round {rnd}: {shown}\n")
                    f.flush()
                    prev = cur
                    rnd += 1
                if _STOP:
                    break
    finally:
        try:
            ring.close()
        except Exception:  # noqa: BLE001 — teardown best-effort
            pass
    print(f"[sample_certus_iostat] stopped after {rnd} rounds -> {args.out}",
          file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()

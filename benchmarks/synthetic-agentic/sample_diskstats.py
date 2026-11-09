#!/usr/bin/env python3
"""sample_diskstats.py — poll host /proc/diskstats and emit renderer [prom] lines.

The synthetic-agentic Tiered-CPU-FS arm spills evicted KV blocks to a filesystem
tier on a real block device (RAID0/XFS at /dev/md0 here). vLLM's served /metrics
exposes the connector's LOGICAL byte movement (kv_offload_store_bytes /
kv_offload_load_bytes) but NOT physical device I/O — the ssd_read_bytes /
ssd_write_bytes counters render_kvprofile plots come from the certus in-process
connector's telemetry, which a black-box `vllm serve` container never exports. So
scrape_prom.py (which only reads /metrics) leaves those two keys empty and the
disk-traffic bars stay flat.

This poller closes that gap for the FS-tier arm the same way scrape_prom.py does
for /metrics: it samples /proc/diskstats for one device on an interval and writes
`[prom] round N: ssd_read_bytes=<delta> ssd_write_bytes=<delta>` lines — the exact
keys render_kvprofile already plots, so no renderer change. Its round numbers run
independently of scrape_prom.py's, which is fine: render merges tokens by round
and sums each key across rounds, and these two keys are disjoint from the /metrics
keys, so the two sidecars fold together cleanly.

A baseline is taken at startup and NOT emitted, so the summed deltas measure only
the driven window — the big model-weights read (HF cache also lives on this
device) happens before the server is ready, i.e. before our baseline, and is
excluded. NB this is WHOLE-DEVICE I/O: any other traffic to the device during the
run (the results/log writes into the same mount) is counted too, but the KV tier
dominates. SPDK devices (the certus arm) bypass the kernel block layer and never
appear in /proc/diskstats — the sibling sample_certus_iostat.py emits the same two
[prom] keys for that arm from the certus-server's GetIoStats shmq op instead.

Usage:
  sample_diskstats.py --mount /mnt/fs-backend-bench --interval 10 --out run.disk.log
  sample_diskstats.py --dev md0 --interval 10 --out run.disk.log
"""

from __future__ import annotations

import argparse
import os
import signal
import sys
import time

# /proc/diskstats: `major minor name reads rd_merged rd_sectors rd_ms writes
# wr_merged wr_sectors wr_ms ...`. Sectors are always 512 bytes for these fields,
# independent of the device's physical/logical block size.
_SECTOR_BYTES = 512
_F_NAME = 2
_F_RD_SECTORS = 5
_F_WR_SECTORS = 9


def _dev_from_mount(mount: str) -> str:
    """Resolve a mountpoint to a /proc/diskstats device basename (e.g. `md0`).

    Reads /proc/mounts (no external tooling) and picks the longest matching
    mountpoint so a nested mount wins over its parent. Strips a `/dev/` prefix."""
    best_src = ""
    best_len = -1
    try:
        with open("/proc/mounts", encoding="utf-8") as f:
            for line in f:
                parts = line.split()
                if len(parts) < 2:
                    continue
                src, mnt = parts[0], parts[1]
                if (mount == mnt or mount.rstrip("/").startswith(mnt.rstrip("/") + "/") or mount.rstrip("/") == mnt.rstrip("/")) and len(mnt) > best_len:
                    best_src, best_len = src, len(mnt)
    except OSError as e:
        print(f"[sample_diskstats] cannot read /proc/mounts: {e}", file=sys.stderr, flush=True)
    if not best_src:
        return ""
    return os.path.basename(best_src)


def scrape(dev: str) -> dict:
    """Return {ssd_read_bytes, ssd_write_bytes} cumulative for `dev`, or {}.

    An unmatched device (name not in /proc/diskstats — e.g. an SPDK device, which
    bypasses the kernel block layer) returns {} so nothing is emitted, rather than
    a stream of zeros that would masquerade as 'ran, no disk I/O'."""
    try:
        with open("/proc/diskstats", encoding="utf-8") as f:
            for line in f:
                parts = line.split()
                if len(parts) <= _F_WR_SECTORS or parts[_F_NAME] != dev:
                    continue
                return {
                    "ssd_read_bytes": float(parts[_F_RD_SECTORS]) * _SECTOR_BYTES,
                    "ssd_write_bytes": float(parts[_F_WR_SECTORS]) * _SECTOR_BYTES,
                }
    except (OSError, ValueError) as e:  # transient/parse — skip this tick
        print(f"[sample_diskstats] read failed: {e}", file=sys.stderr, flush=True)
    return {}


_STOP = False


def _handle_stop(signum, _frame):
    global _STOP
    _STOP = True


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--dev", help="/proc/diskstats device basename (e.g. md0)")
    g.add_argument("--mount", help="mountpoint to resolve to a device")
    ap.add_argument("--interval", type=float, default=10.0, help="seconds between samples")
    ap.add_argument("--out", required=True, help="file to append [prom] lines to")
    args = ap.parse_args()

    dev = args.dev or _dev_from_mount(args.mount)
    if not dev:
        print(f"[sample_diskstats] no device for mount={args.mount!r}; nothing sampled",
              file=sys.stderr, flush=True)
        return

    signal.signal(signal.SIGTERM, _handle_stop)
    signal.signal(signal.SIGINT, _handle_stop)

    fmt = lambda v: (str(int(v)) if float(v).is_integer() else repr(v))  # noqa: E731

    with open(args.out, "a", encoding="utf-8") as f:
        # Baseline (not emitted): exclude everything before the server was ready
        # (model-weights read from the HF cache on this same device, etc.).
        prev = scrape(dev)
        if not prev:
            print(f"[sample_diskstats] device {dev!r} not in /proc/diskstats; nothing sampled",
                  file=sys.stderr, flush=True)
            return
        print(f"[sample_diskstats] dev={dev} url=/proc/diskstats", file=sys.stderr, flush=True)

        rnd = 0
        while True:
            waited = 0.0
            while waited < args.interval and not _STOP:
                time.sleep(0.2)
                waited += 0.2
            cur = scrape(dev)
            if cur:
                delta = {k: max(0.0, cur.get(k, 0.0) - prev.get(k, 0.0)) for k in cur}
                shown = " ".join(f"{k}={fmt(delta[k])}" for k in sorted(delta))
                f.write(f"[prom] round {rnd}: {shown}\n")
                f.flush()
                prev = cur
                rnd += 1
            if _STOP:
                break
    print(f"[sample_diskstats] stopped after {rnd} rounds -> {args.out}",
          file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()

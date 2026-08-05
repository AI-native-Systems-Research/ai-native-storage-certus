#!/usr/bin/env python3
"""Summarise sweep rows produced by sweep-row.py.

Usage: sweep-summary.py <sweep.tsv> [<sweep.tsv> ...]

Accepting several files is deliberate: concatenating the local leg's TSV with the
remote leg's puts both in one table, which is the comparison the sweep exists to
make. Rows are grouped by (label, batch, workers, inflight).

## What the sweep is actually testing

Throughput and latency are not independent measurements here. For a load generator
holding a fixed number of requests in flight,

    throughput = bytes_in_flight / per_batch_latency

holds identically (Little's law). So a config's GB/s is fully determined by its
in-flight bytes and its latency, and "add more parallelism" only buys throughput
while latency stays flat. Two readings:

  * GB/s rises with concurrency, p50 flat  -> there was queueing headroom; the path
    was in-flight-limited and more parallelism is the right fix.
  * GB/s flat, p50 rises in proportion     -> the added work is queueing behind a
    serialized resource; parallelism cannot help and the serialization is the bug.

The `fixed-footprint` section is the discriminator. Configs there hold in-flight
BYTES constant while varying request concurrency (smaller batches, more of them).
If throughput moves along that row, concurrency itself mattered; if it does not,
only bytes in flight did.
"""

import statistics as st
import sys
from collections import defaultdict

SUFFIX = {"K": 2**10, "M": 2**20, "G": 2**30}


def objbytes(s: str) -> int:
    """Parse `64K` / `4M` / a bare byte count."""
    s = s.strip()
    if s and s[-1].upper() in SUFFIX:
        return int(s[:-1]) * SUFFIX[s[-1].upper()]
    return int(s)


def load(paths):
    rows = []
    for path in paths:
        with open(path) as fh:
            for line in fh:
                line = line.rstrip("\n")
                if not line.strip() or line.startswith("#"):
                    continue
                parts = line.split("\t")
                if len(parts) != 14:
                    print(f"# skipping malformed row in {path}: {line!r}",
                          file=sys.stderr)
                    continue
                (b, w, f, r, label, objsize, gbps, p50, p99, el,
                 ok, bad, lops, vf) = parts
                rows.append(dict(
                    b=int(b), w=int(w), f=int(f), r=int(r), label=label,
                    objsize=objsize, gbps=float(gbps), p50=float(p50),
                    p99=float(p99), el=float(el), ok=int(ok), bad=int(bad),
                    lops=int(lops), vf=int(vf),
                ))
    return rows


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    rows = load(sys.argv[1:])
    if not rows:
        print("no sweep rows parsed")
        return 0

    groups = defaultdict(list)
    for row in rows:
        groups[(row["label"], row["b"], row["w"], row["f"])].append(row)

    # --- Main table -------------------------------------------------------
    hdr = ("label", "batch", "wrk", "infl", "concur", "flight_MiB",
           "n", "GB/s", "median", "cv%", "p50_ms", "keys/s")
    widths = (22, 6, 5, 5, 7, 11, 4, 8, 8, 6, 8, 9)
    print("  " + " ".join(f"{h:>{wd}}" for h, wd in zip(hdr, widths)))
    warnings = []
    for (label, b, w, f), g in sorted(groups.items()):
        gb = [x["gbps"] for x in g]
        mean = st.mean(gb)
        cv = (st.stdev(gb) / mean * 100) if len(gb) > 1 and mean else 0.0
        flight = w * f * b * objbytes(g[0]["objsize"]) / 2**20
        vals = (label, b, w, f, w * f, f"{flight:.0f}", len(gb),
                f"{mean:.3f}", f"{st.median(gb):.3f}",
                f"{cv:.1f}" if len(gb) > 1 else "-",
                f"{st.mean(x['p50'] for x in g) / 1000:.2f}",
                f"{st.mean(x['ok'] / x['el'] for x in g):.0f}")
        print("  " + " ".join(f"{str(v):>{wd}}" for v, wd in zip(vals, widths)))
        bad = sum(x["bad"] for x in g)
        lops = sum(x["lops"] for x in g)
        vf = sum(x["vf"] for x in g)
        if bad or lops or vf:
            warnings.append(
                f"{label} batch={b} w={w} infl={f}: keys_failed={bad} "
                f"local_read_ops={lops} verify_failures={vf}")

    if warnings:
        print()
        print("  WARNINGS (a nonzero local_read_ops on a remote leg means some hits")
        print("  were served locally, so that row is not measuring the fabric):")
        for line in warnings:
            print(f"    {line}")

    # --- Concurrency response --------------------------------------------
    print()
    print("  === Concurrency response (vs the lowest-concurrency config per label) ===")
    by_label = defaultdict(list)
    for (label, b, w, f), g in groups.items():
        by_label[label].append((
            w * f, b, w, f,
            st.mean(x["gbps"] for x in g),
            st.mean(x["p50"] for x in g),
            w * f * b * objbytes(g[0]["objsize"]) / 2**20,
        ))
    for label, pts in sorted(by_label.items()):
        pts.sort()
        c0, b0, w0, f0, g0, l0, m0 = pts[0]
        print(f"    {label}: ref concur={c0} (batch={b0} wrk={w0} infl={f0}, "
              f"{m0:.0f} MiB) = {g0:.3f} GB/s, p50 {l0 / 1000:.2f} ms")
        for c, b, w, f, gv, lv, m in pts[1:]:
            gr, lr = gv / g0, lv / l0
            if c == c0:
                # Same request concurrency, more bytes per request. This axis says
                # nothing about parallelism — only whether a bigger batch alone
                # buys anything, and a proportional latency rise at flat
                # throughput is what "it does not" looks like.
                verdict = ("bigger batch helped" if gr > 1.15
                           else "bigger batch alone: no gain")
            elif gr > 1.15:
                verdict = "SCALED — had headroom"
            elif lr > 1.5 * gr:
                # Latency tracking concurrency 1:1 at flat throughput is the
                # signature of a serialized resource: extra requests only queue.
                verdict = "queued — serialized"
            else:
                verdict = "flat"
            print(f"      concur x{c / c0:<5.1f} batch={b:<4} wrk={w:<3} infl={f:<3} "
                  f"{m:>5.0f} MiB : GB/s x{gr:<5.2f} p50 x{lr:<5.2f}  {verdict}")

    # --- Fixed-footprint discriminator -----------------------------------
    print()
    print("  === Fixed in-flight bytes, varying request concurrency ===")
    print("  (throughput moving along a row means concurrency itself mattered;")
    print("   throughput flat means only bytes-in-flight ever mattered)")
    by_flight = defaultdict(list)
    for (label, b, w, f), g in groups.items():
        flight = round(w * f * b * objbytes(g[0]["objsize"]) / 2**20)
        by_flight[(label, flight)].append((
            w * f, b, w, f, st.mean(x["gbps"] for x in g),
            st.mean(x["p50"] for x in g)))
    printed = False
    for (label, flight), pts in sorted(by_flight.items()):
        if len(pts) < 2:
            continue
        printed = True
        pts.sort()
        print(f"    {label} @ {flight} MiB in flight:")
        for c, b, w, f, gv, lv in pts:
            print(f"      concur {c:<4} (batch={b:<4} wrk={w:<3} infl={f:<3}) "
                  f"{gv:.3f} GB/s  p50 {lv / 1000:.2f} ms")
    if not printed:
        print("    (no two configs shared an in-flight-byte total — add a")
        print("     fixed-footprint axis, e.g. 64:4:4;32:4:8;16:4:16;8:4:32)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

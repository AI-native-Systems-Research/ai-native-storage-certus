#!/usr/bin/env python3
"""Segment a width-by-depth profile into a `branching` profile — the T074 rule.

Consumes the JSON that `width_profile.py` produces, so the rule can be re-derived
without re-reading the traces. See `research.md` § The branching segmentation rule
for the derivation; this is its executable form.

The rule in four steps:

1. **Clip.** `f(d) = max(1, w(d)/w(d-1))`. Schema rule 8 requires fanout >= 1 at
   every depth, so an observed *decrease* is not a fanout — it is censoring by
   session retirement, and it carries no information about branching. An observed
   *increase* cannot be produced by censoring, so it is genuine and is a **lower
   bound** on the true fanout.

2. **Merge at the realisation resolution.** The generator realises a non-integer
   mean fanout by randomised rounding per node, so the fanout it actually produces
   at a depth of width `w` has standard error `sqrt(frac(1-frac)/w)` on `f`. Two
   adjacent depths are merged unless their fanouts differ by more than `Z` such
   standard errors. No jump-ratio threshold appears anywhere: the resolution comes
   from the generator's own mechanism, and a distinction finer than it would be
   describing noise the generator cannot reproduce.

3. **Fold the near-root levels** into `roots.count` while the leading segment's
   fanout would otherwise drive occupancy below the FR-009f floor (FR-055c).

4. **Fit only where the profile is not a survival curve.** Segmentation stops at
   the depth where cumulative retention falls under `--retention`, because a
   segment's fanout is a *product* over its depths and censoring compounds through
   it. Beyond that depth nothing is fitted and the observed width is reported as a
   lower bound. `roots.count` is exempt: it is a single width reading rather than a
   product, so it is taken at the fold boundary whatever the retention there, with
   that retention reported beside it.

Usage:
    /tmp/pqenv/bin/python segment.py profile.json [--z 3.0] [--retention 0.99]
        [--target-occupancy 4.0]
"""

import argparse
import json
import math
import sys


def clipped_fanouts(widths):
    """`max(1, w(d)/w(d-1))` for d >= 1, and how many were clipped."""
    out, clipped = [], 0
    for i in range(1, len(widths)):
        if widths[i - 1] <= 0:
            out.append(1.0)
            continue
        r = widths[i] / widths[i - 1]
        if r < 1.0:
            clipped += 1
            r = 1.0
        out.append(r)
    return out, clipped


def rounding_se(f, w):
    """Standard error of a realised fanout `f` at a depth of width `w`.

    Randomised rounding gives each of the `w` nodes `floor(f)` or `ceil(f)`
    children, taking the higher with probability `frac = f - floor(f)`, so the
    realised mean is a Bernoulli average over `w` draws: `sd = sqrt(frac(1-frac)/w)`.
    Confirmed against the generator — a uniform 1.05 profile at width 200 predicts
    1.5% and measures 1.4% at p90.

    Floored at half a child per node, since a segment whose width is tiny cannot
    resolve anything and must not claim a small error.
    """
    w = max(w, 1)
    frac = f - math.floor(f)
    var = frac * (1.0 - frac) / w
    return max(math.sqrt(var), 0.5 / w)


def segment_occupancy(depths, seg):
    """Mean sessions per distinct node over a segment's depths.

    FR-055b requires the fit report to state the occupancy each width ratio was
    measured at, so that a fanout near 1 is not mistaken for a genuinely linear
    trunk when it is really one session per path.
    """
    vals = []
    for d in range(seg["from_depth"], seg["to_depth"] + 1):
        if d < len(depths) and depths[d]["width"] > 0:
            vals.append(depths[d]["sessions"] / depths[d]["width"])
    return sum(vals) / len(vals) if vals else 0.0


def segment(widths, z):
    """Bottom-up merge of adjacent depths into constant-fanout segments.

    Every depth starts as its own segment and the most consistent adjacent pair is
    merged repeatedly, stopping once the best available merge is distinguishable at
    `z` standard errors. Merging rather than splitting because the null hypothesis
    is the one the traces support overwhelmingly — a flat trunk — so the procedure
    starts from the data and only keeps distinctions it can defend.
    """
    fanouts, clipped = clipped_fanouts(widths)
    if not fanouts:
        return [], clipped
    # A segment is [start_depth, end_depth, [ratios], [widths]] where a ratio at
    # index i is the step from depth start+i to start+i+1.
    segs = [[i + 1, i + 1, [f], [widths[i]]] for i, f in enumerate(fanouts)]

    def fanout_of(seg):
        # The geometric mean of the ratios inside, as the contract specifies.
        return math.exp(sum(math.log(r) for r in seg[2]) / len(seg[2]))

    def merge_z(a, b):
        """Standard errors between two adjacent segments' fanouts."""
        fa, fb = fanout_of(a), fanout_of(b)
        wa = sum(a[3]) / len(a[3])
        wb = sum(b[3]) / len(b[3])
        se = math.sqrt(
            (rounding_se(fa, wa) ** 2) / len(a[2]) + (rounding_se(fb, wb) ** 2) / len(b[2])
        )
        return abs(fa - fb) / se if se > 0 else float("inf")

    while len(segs) > 1:
        zs = [merge_z(segs[i], segs[i + 1]) for i in range(len(segs) - 1)]
        best = min(range(len(zs)), key=lambda i: zs[i])
        if zs[best] > z:
            break
        a, b = segs[best], segs[best + 1]
        segs[best : best + 2] = [[a[0], b[1], a[2] + b[2], a[3] + b[3]]]
    return [
        {
            "from_depth": s[0],
            "to_depth": s[1],
            "fanout": fanout_of(s),
            "depths": len(s[2]),
        }
        for s in segs
    ], clipped


def retention_limit(depths, floor):
    """Largest depth whose cumulative retention is still at or above `floor`."""
    s0 = depths[0]["survivors"] if depths else 0
    if s0 <= 0:
        return 0
    limit = 0
    for x in depths:
        if x["survivors"] / s0 < floor:
            break
        limit = x["depth"]
    return limit


def fold_root(segs, widths, sessions_at_p99, p99_depth, target):
    """Fold leading levels into `roots.count` per FR-055c.

    A near-root segment with an enormous fanout cannot be expressed as trunk
    branching: FR-009f's occupancy floor is `sessions_per_window / paths(d)`, and a
    single level of four orders of magnitude drives it below any useful value
    immediately. So leading segments are absorbed — their width becoming
    `roots.count` — for exactly as long as that is what keeps occupancy at the
    fitted sharing depth above the floor.

    This is FR-055c derived rather than asserted: the fold goes exactly as deep as
    the occupancy floor requires and no deeper, so a trace with a genuinely wide
    but shallow trunk keeps it.
    """
    boundary = 0
    roots = widths[0] if widths else 1
    kept = list(segs)
    while kept:
        paths = roots
        for s in kept:
            span = min(s["to_depth"], p99_depth) - s["from_depth"] + 1
            if span > 0:
                paths *= s["fanout"] ** span
        occ = sessions_at_p99 / paths if paths > 0 else 0.0
        if occ >= target:
            break
        first = kept[0]
        # Only a *near-root* segment may be folded; a deep fanout event cannot be
        # expressed as roots at all, and the fold must stop rather than pretend.
        if first["from_depth"] > p99_depth or len(kept) == 1:
            break
        boundary = first["to_depth"]
        roots = widths[min(boundary, len(widths) - 1)]
        kept = kept[1:]
    return boundary, roots, kept


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("profile")
    ap.add_argument("--z", type=float, default=3.0)
    ap.add_argument("--retention", type=float, default=0.99)
    ap.add_argument("--target-occupancy", type=float, default=4.0)
    args = ap.parse_args()

    p = json.load(open(args.profile))
    depths = p["depths"]
    if not depths:
        sys.exit("empty profile")
    widths_all = [x["width"] for x in depths]
    limit = retention_limit(depths, args.retention)

    # Segment only the uncensored prefix: a segment's fanout is a product over its
    # depths, so censoring compounds through it.
    segs, clipped = segment(widths_all[: limit + 1], args.z)

    p99_depth = max(limit, 0)
    sessions_at_p99 = depths[p99_depth]["sessions"]
    boundary, roots, kept = fold_root(
        segs, widths_all, sessions_at_p99, p99_depth, args.target_occupancy
    )
    retention_at_boundary = (
        depths[boundary]["survivors"] / depths[0]["survivors"] if depths[0]["survivors"] else 0.0
    )
    peak = max(range(len(widths_all)), key=lambda i: widths_all[i])

    print(f"{p['trace']}  bs={p['block_size']}  {p['source_class']}")
    print(
        f"  depths={len(widths_all)}  fitted={max(limit, 0)}  censored_ratios={clipped}  "
        f"chronological={p['chronological']}"
    )
    print(
        f"  root boundary depth {boundary}, roots.count {roots} "
        f"(retention there {retention_at_boundary:.3f})"
    )
    print(f"  trunk segments: {len(kept)}")
    for s in kept[:10]:
        occ = segment_occupancy(depths, s)
        # FR-055b: a fanout near 1 measured at occupancy near 1 is not evidence of
        # a linear trunk, so the two are never printed apart.
        flag = "  [occupancy ~1: ratio uninformative]" if occ < 2.0 else ""
        print(
            f"    from_depth {s['from_depth']:>6}  to {s['to_depth']:>6}  "
            f"fanout {s['fanout']:.4f}  over {s['depths']} depths  "
            f"occupancy {occ:.1f}{flag}"
        )
    if len(kept) > 10:
        print(f"    ... {len(kept) - 10} more")
    print(
        f"  not fitted beyond depth {max(limit, 0)}: observed width peaks at "
        f"{widths_all[peak]} at depth {peak}, a lower bound thereafter"
    )


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Why FR-015 seeds a pool from the equilibrium residual-life distribution.

Run with no arguments. Needs numpy.

    python3 research/population/seeding.py

THE CLAIM
---------

At `t = 0` a pool must be seeded by giving each instance a *residual* life drawn
from the equilibrium residual-life distribution

    P(R <= x) = (1/E[L]) * integral_0^x (1 - F(u)) du

and **not** a full life drawn from the lifetime distribution `F` itself. Seeding
from `F` gives every instance the same birthday, so the pool turns over in
synchronised cohorts and the key-space churn the cache sees is periodic.

`residual_sampler` below is the algorithm FR-015 requires, and it is general over
any lifetime distribution: take a **length-biased** pick of a lifetime, then a
uniform point within it. That is exactly the equilibrium construction, and it
needs no closed form, which matters because a description may write a lifetime as
any of five distribution kinds including an empirical sample set.

WHAT THE OUTPUT SHOWS
---------------------

Steady-state churn is `N * window / E[L]` — 2.0 per window for the pool below.
Seeded from the lifetime distribution, the first three windows see **zero churn**
(6000 virtual seconds in which no key is ever replaced), then a burst. Eight full
lifetimes later the modulation is still around 40% peak-to-trough. Seeded from
residual life, churn is flat at 2.0 from `t = 0`.

For an exponential lifetime the two seedings coincide, because exponential is
memoryless and its equilibrium residual life is the same exponential. The second
table checks that, and it is a useful control: a bug in the residual sampler
would show up there as a difference where none should exist.

CORRECTED RELATIVE TO ~scooter/popsim/seed.py
---------------------------------------------

That version's exponential comparison ran **one seed per arm**, and its residual
sampler consumed 200,000 draws from the *same* generator before the main loop, so
the two arms were different realisations rather than a controlled comparison. Its
means came out 1.96 and 2.32 — about 2.4 standard errors apart, which reads as a
real difference but is noise, and which contradicts the memorylessness it was
meant to demonstrate. Here each arm gets its own generator and the result is
averaged over many seeds.

Also: that version used `np.maximum(1.0, normal(...))`, which *clamps*. The
shipped implementation truncates (FR-008). At 10 sigma from the mean the
difference is immaterial, but it is not the same operation, so this version
truncates by resampling.
"""

from __future__ import annotations

import heapq

import numpy as np

N = 10
MU, SIGMA = 10_000.0, 1_000.0
WINDOW = 2_000.0
GENERATIONS = 8
NSEEDS = 40


def truncated_normal(rng, mu, sigma, lo, m):
    """Draw m values from normal(mu, sigma) truncated below at lo.

    Rejection rather than clamping: clamping would put an atom at `lo`.
    """
    out = np.empty(m)
    filled = 0
    while filled < m:
        cand = rng.normal(mu, sigma, m)
        cand = cand[cand > lo]
        take = min(len(cand), m - filled)
        out[filled:filled + take] = cand[:take]
        filled += take
    return out


def residual_sampler(rng, draw, n_grid=200_000):
    """Equilibrium residual life: length-biased lifetime, then a uniform point in it."""
    grid = draw(n_grid)
    p = grid / grid.sum()

    def sample(m):
        idx = rng.choice(len(grid), size=m, p=p)
        return grid[idx] * rng.uniform(0.0, 1.0, m)

    return sample


def churn_windows(draw, rng, t_end, seeding, n=N, window=WINDOW):
    """Exact pool of n with immediate replacement. Returns replacements per window."""
    init = draw(n) if seeding == "lifetime" else residual_sampler(rng, draw)(n)
    deaths = list(init)
    heapq.heapify(deaths)
    events = []
    while deaths[0] < t_end:
        d = heapq.heappop(deaths)
        events.append(d)
        heapq.heappush(deaths, d + draw(1)[0])
    counts, _ = np.histogram(np.array(events), bins=np.arange(0.0, t_end + window, window))
    return counts


def modulation(row):
    """Peak-to-trough modulation, (max - min) / (max + min)."""
    hi, lo = float(np.max(row)), float(np.min(row))
    return 0.0 if hi + lo == 0 else (hi - lo) / (hi + lo)


def by_generation(kind):
    """Mean churn per window, per generation, for both seedings."""
    per_gen = int(round(MU / WINDOW))
    t_end = GENERATIONS * MU
    rows = {"lifetime": [], "residual": []}
    for seeding in rows:
        acc = []
        for s in range(NSEEDS):
            rng = np.random.default_rng(s)
            if kind == "normal":
                def draw(m, r=rng):
                    return truncated_normal(r, MU, SIGMA, 1.0, m)
            else:
                def draw(m, r=rng):
                    return r.exponential(MU, m)
            acc.append(churn_windows(draw, rng, t_end, seeding))
        a = np.mean(np.array(acc, float), axis=0)
        rows[seeding] = [a[g * per_gen:(g + 1) * per_gen] for g in range(GENERATIONS)]
    return rows


def report(kind, note):
    steady = N * WINDOW / MU
    print(f"\n{kind} lifetime (mean {MU:g}"
          f"{f', sigma {SIGMA:g}' if kind == 'normal' else ''}), "
          f"exact pool of N={N}, {WINDOW:g}-vs windows")
    print(f"{note}")
    print(f"Steady state is N*window/E[L] = {steady:g} replacements per window. "
          f"Mean of {NSEEDS} seeds.\n")
    rows = by_generation(kind)
    print(f"{'gen':>4}  {'seeded from lifetime':>34} {'mod':>6}   "
          f"{'seeded from residual life':>34} {'mod':>6}")
    for g in range(GENERATIONS):
        fr, rs = rows["lifetime"][g], rows["residual"][g]
        print(f"{g:>4}  {np.array2string(fr, precision=1, floatmode='fixed'):>34} "
              f"{modulation(fr):6.2f}   "
              f"{np.array2string(rs, precision=1, floatmode='fixed'):>34} "
              f"{modulation(rs):6.2f}")
    first, last = rows["lifetime"][0], rows["lifetime"][-1]
    zero = int(np.sum(first < 0.5))
    print(f"\n  seeded from lifetime: {zero} of the first {len(first)} windows below 0.5 "
          f"({zero * WINDOW:g} vs of near-zero churn), "
          f"modulation still {modulation(last):.2f} after {GENERATIONS} generations")
    print(f"  seeded from residual: modulation {modulation(rows['residual'][0]):.2f} "
          f"in generation 0, {modulation(rows['residual'][-1]):.2f} at the end")


def main() -> None:
    report("normal", "A normal lifetime has a mode, so a shared birthday synchronises the pool.")
    report("exponential",
           "CONTROL: exponential is memoryless, so the two seedings must agree. "
           "A difference here would mean the residual sampler is wrong.")


if __name__ == "__main__":
    main()

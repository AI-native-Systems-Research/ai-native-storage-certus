#!/usr/bin/env python3
"""Why FR-016 forbids a feedback controller for population regulation.

Run with no arguments. Needs numpy.

    python3 research/population/controller.py

THE HEADLINE, AND A CORRECTION
------------------------------

An earlier version of this study concluded that an integral controller carries a
"+8-23% positive population bias from the non-negative creation rate". **That
conclusion was wrong**, and it was wrong in a way worth recording, because it was
used to justify a requirement.

The controller regulates the mean *well*. Measured against the reference C
implementation at `~scooter/birth-death/main.c` (ALPHA=0.1, Ts=0.01,
MEAN_TTL=1.0, TARGET=100), whose own `xystats` reports `y_mean = 99.1974` over
the full run, this port lands at 99.16 — a bias under 1%. The non-negative rate
clamp is not the mechanism either: at those parameters it **never fires at all**,
so it cannot bias anything.

The +8-23% figure came from a parameter sweep that was badly scaled, not from a
property of feedback. The per-sample numerator gain is `p1 ~ Ts^2/2`, so moving
from Ts=0.01 to T=1 multiplies the loop gain by about 10^4 while the required
birth rate `N/E[L]` falls by 200x. The loop is then hugely over-gained, it
oscillates, the commanded rate spends half its time negative, and the clamp
rectifies it. That is a badly tuned loop, not an argument against loops.

THE ARGUMENT THAT ACTUALLY SURVIVES
-----------------------------------

The same measurements support a different and better reason, visible in the
`var/mean` column below:

  * A real population of independent birth-death objects is M/G/inf, for which
    `var = mean` exactly. The free-running Poisson form reproduces that, at
    var/mean = 1.03.
  * The controller drives var/mean to **0.48** — it halves the dispersion — and
    does so at every gain tried, because suppressing fluctuation is what a
    regulator is *for*.

FR-013 asks for "a live count whose relative spread falls as the inverse square
root of the target", which is Poisson. So a controller does not fail to do its
job; it does its job, and its job is the opposite of what this instrument needs.
Tight regulation is a fidelity loss, not a gain. That is why FR-016 forbids the
approach rather than prescribing a gain — and it is a stronger reason than the
one it replaces, because it follows from what the tool is for instead of from an
implementation artifact.

Secondarily, a controller exposes two tuning parameters (`alpha` and the sampling
period) whose effect ranges from excellent to catastrophic. FR-005 requires a
workload description to be portable across clusters unchanged, so it must not
carry them.

WHAT IS FAITHFUL TO WHAT
------------------------

`sim()` below is a port of `~scooter/birth-death/main.c`. Both realise the same
transfer function — a ZOH-discretised integrator cascaded with a first-order
low-pass at pole `alpha`:

    A  = exp(-alpha*Ts)
    p1 = (A-1)/alpha + Ts
    p2 = (1 - (1+alpha*Ts)*A)/alpha
    y_k = (1+A) y_{k-1} - A y_{k-2} + p1 u_{k-1} + p2 u_{k-2}      u = N* - n

The C version writes it as a state-space biquad and this one as the difference
equation; expanding the former gives the latter exactly.

**There is no clamp inside the reference controller, by design.** Its state
variables are free to produce a negative rate, and nothing rectifies them. The
non-negativity of births is enforced only *indirectly*, by the birth test
`y * (t - t0) >= 1.0`, which a negative `y` simply never satisfies. That is the
right place for it: a rate that cannot be acted on is still the correct control
signal, and suppressing it in the state would be the classic windup mistake.

This port matches that. `y` goes into the controller state unmodified; the
`max(y, 0)` appearing below is used only in the birth test, where it is provably
equivalent to using `y` raw — for `y < 0` and `t > t0`, `y*(t-t0)` is negative and
`0*(t-t0)` is zero, and neither reaches 1. Verified empirically as well: both
forms agree to every digit of the mean and variance at alpha in {0.01, 0.1, 1}.

Two switches select the *old* Python behaviour so the retracted result is
reproducible rather than merely described:

  `feedback_clamped`     feed the clamped rate back into the controller state
                         instead of the commanded one — i.e. the windup mistake
                         the reference deliberately avoids. Inert at sane
                         parameters, because the clamp never fires there.
  `one_birth_per_tick`   the C rule above, at most one birth per tick. The
                         alternative spaces births deterministically at 1/y,
                         which leaves the mean alone but inflates the variance by
                         25x, because deterministic spacing is not a Poisson
                         process.
"""

from __future__ import annotations

import heapq
import math

import numpy as np

# The reference C implementation's parameters.
TARGET = 100
TAU = 1.0
TS = 0.01
T_END = 300.0
SEEDS = range(5)


def sim(alpha, Ts=TS, tau=TAU, target=TARGET, t_end=T_END, seed=0,
        feedback_clamped=False, one_birth_per_tick=True):
    """Closed loop: controller -> birth rate -> births; deaths by exponential TTL."""
    rng = np.random.default_rng(seed)
    A = math.exp(-alpha * Ts)
    p1 = (A - 1.0) / alpha + Ts
    p2 = (1.0 - (1.0 + alpha * Ts) * A) / alpha

    deaths: list[float] = []
    y1 = y2 = u1 = u2 = 0.0
    t0 = 0.0
    out, clipped, nk, k = [], 0, 0, 0

    while (k + 1) * Ts <= t_end:
        t = (k + 1) * Ts
        while deaths and deaths[0] <= t:
            heapq.heappop(deaths)
        n = len(deaths)
        out.append(n)

        u = target - n
        y = (1.0 + A) * y1 - A * y2 + p1 * u1 + p2 * u2
        nk += 1
        y_eff = y
        if y < 0.0:
            clipped += 1
            y_eff = 0.0                      # a birth rate cannot be negative

        if one_birth_per_tick:
            if n < 1 or (t > t0 and y_eff * (t - t0) >= 1.0):
                heapq.heappush(deaths, t + rng.exponential(tau))
                t0 = t
        else:
            while y_eff > 0:
                tn = t0 + 1.0 / y_eff
                if tn > t:
                    break
                while deaths and deaths[0] <= tn:
                    heapq.heappop(deaths)
                heapq.heappush(deaths, tn + rng.exponential(tau))
                t0 = tn

        y2, y1 = y1, (y_eff if feedback_clamped else y)
        u2, u1 = u1, u
        k += 1

    x = np.array(out, float)
    x = x[len(x) // 10:]                     # drop the startup transient
    return x.mean(), x.var(), clipped / max(nk, 1)


def free_poisson(seed=0, Ts=TS, tau=TAU, target=TARGET, t_end=T_END):
    """No feedback: births are a Poisson process at the constant rate N/E[L]."""
    rng = np.random.default_rng(seed)
    rate = target / tau
    deaths: list[float] = []
    out, k = [], 0
    nxt = rng.exponential(1.0 / rate)
    while (k + 1) * Ts <= t_end:
        t = (k + 1) * Ts
        while nxt <= t:
            heapq.heappush(deaths, nxt + rng.exponential(tau))
            nxt += rng.exponential(1.0 / rate)
        while deaths and deaths[0] <= t:
            heapq.heappop(deaths)
        out.append(len(deaths))
        k += 1
    x = np.array(out, float)
    x = x[len(x) // 10:]
    return x.mean(), x.var(), 0.0


def average(fn, **kw):
    rows = [fn(seed=s, **kw) for s in SEEDS]
    return tuple(float(np.mean([r[i] for r in rows])) for i in range(3))


def main() -> None:
    print(f"TARGET={TARGET}, lifetime Exp(mean={TAU}), Ts={TS}, t_end={T_END}, "
          f"{len(list(SEEDS))} seeds, startup transient discarded")
    print("Reference: an uncontrolled M/G/inf population has var = mean exactly.")
    print("Reference C run (~scooter/birth-death, its own xystats): y_mean = 99.1974\n")

    hdr = f"{'form':46s} {'mean':>8} {'var':>10} {'var/mean':>9} {'bias':>8} {'clamped':>8}"
    print(hdr)
    print("-" * len(hdr))

    m, v, c = average(free_poisson)
    print(f"{'free-running Poisson (what FR-013 asks for)':46s} "
          f"{m:8.2f} {v:10.2f} {v/m:9.3f} {100*(m-TARGET)/TARGET:+7.2f}% {100*c:7.1f}%")

    for alpha in (0.01, 0.1, 1.0):
        m, v, c = average(sim, alpha=alpha)
        print(f"{'controller alpha=' + f'{alpha:g}' + ' (faithful to main.c)':46s} "
              f"{m:8.2f} {v:10.2f} {v/m:9.3f} {100*(m-TARGET)/TARGET:+7.2f}% {100*c:7.1f}%")

    print("\nThe two switches, at the same parameters — neither explains a mean bias:")
    for label, kw in [
        ("state clamp fed back (the suspected bug)", dict(feedback_clamped=True)),
        ("deterministic 1/y birth spacing", dict(one_birth_per_tick=False)),
        ("both", dict(feedback_clamped=True, one_birth_per_tick=False)),
    ]:
        m, v, c = average(sim, alpha=0.1, **kw)
        print(f"{'  ' + label:46s} {m:8.2f} {v:10.2f} {v/m:9.3f} "
              f"{100*(m-TARGET)/TARGET:+7.2f}% {100*c:7.1f}%")

    print("\nAnd the badly-scaled sweep that produced the retracted +8-23% figure.")
    print("N=50, tau=100, T=1: the same alpha*tau as above, but ~10^4 the loop gain.")
    for alpha in (0.1, 0.01, 0.001):
        m, v, c = average(sim, alpha=alpha, Ts=1.0, tau=100.0, target=50,
                          t_end=60000.0, one_birth_per_tick=False,
                          feedback_clamped=True)
        print(f"{'  over-gained alpha=' + f'{alpha:g}':46s} {m:8.2f} {v:10.2f} "
              f"{v/m:9.3f} {100*(m-50)/50:+7.2f}% {100*c:7.1f}%")
    print("\nThose last rows are a badly tuned loop, not a property of feedback.")


if __name__ == "__main__":
    main()

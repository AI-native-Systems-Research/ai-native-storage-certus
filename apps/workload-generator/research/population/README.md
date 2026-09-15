# Population dynamics: the evidence behind FR-013 to FR-016

Two studies, both runnable with `python3` and `numpy` and nothing else. They
are research provenance, not part of the test gate: they justify requirements
rather than check the implementation.

```
python3 research/population/controller.py    # why FR-016 forbids a controller
python3 research/population/seeding.py       # why FR-015 seeds from residual life
```

They exist because Constitution Principle IX requires a claim to be traceable
to the measurement behind it, and until now these measurements lived only in an
untracked home directory. Ported from `~scooter/popsim/` (`sim.py`, `sim2.py`,
`sim3.py`, `seed.py`, `seed2.py`), with corrections noted below and in each
file's header.

The reference implementation of the controller is **`~scooter/birth-death/`**,
in C (`main.c`), with its own analysis tool `xystats`. That is the authority;
the Python here is a port checked against it.

## FR-016 — the recorded rationale was WRONG, and is now corrected

**Retracted.** The spec and the constitution previously said an integral
controller "carries an 8-23% positive population bias from the non-negative
creation rate". Both halves of that are false:

- **The controller regulates the mean well.** The C reference at ALPHA=0.1,
  Ts=0.01, MEAN_TTL=1.0, TARGET=100 reports `y_mean = 99.1974` from its own
  `xystats`. This port lands at 99.16, a bias under 1%, at every gain tried.
- **The clamp is not the mechanism.** At those parameters the commanded rate
  never goes negative, so the non-negative constraint **never binds at all**
  and cannot bias anything.

The +8-23% came from a badly-scaled parameter sweep, not from feedback. The
per-sample numerator gain is `p1 ~ Ts²/2`, so moving Ts from 0.01 to 1
multiplies the loop gain by ~10⁴ while the required birth rate `N/E[L]` falls
200×. The loop is then wildly over-gained: it oscillates, the commanded rate is
negative about half the time, and the clamp rectifies it. `controller.py`
reproduces those rows so the retracted result is checkable rather than merely
described.

**The argument that survives, and it is a better one.** The same measurements
show what a regulator actually does here:

| form | mean | var | var/mean |
| --- | --- | --- | --- |
| free-running Poisson | 100.48 | 103.70 | **1.03** |
| controller α=0.01 | 99.20 | 47.58 | **0.48** |
| controller α=0.1 | 99.16 | 47.93 | **0.48** |
| controller α=1 | 99.18 | 47.96 | **0.48** |

An uncontrolled population of independent birth-death objects is M/G/∞, for
which `var = mean` exactly. The free-running form reproduces that. The
controller **halves the dispersion**, at every gain, because suppressing
fluctuation is what a regulator is for.

FR-013 asks for "a live count whose relative spread falls as the inverse square
root of the target" — that is Poisson. So the controller does not fail at its
job; it succeeds, and its job is the opposite of what this instrument needs.
Tight regulation is a fidelity loss. That is why FR-016 forbids the approach
rather than prescribing a gain, and it is a stronger reason than the one it
replaces because it follows from the tool's purpose rather than from an
implementation artifact.

Secondarily: a controller exposes `alpha` and a sampling period whose effect
ranges from excellent to catastrophic (the over-gained rows reach +23% bias and
15× the target variance). FR-005 requires a description to be portable across
clusters unchanged, so it must not carry tuning parameters.

**The reference controller has no clamp inside it, by design.** Its state
variables are free to produce a negative rate, and nothing rectifies them.
Non-negativity of births is enforced only *indirectly*, by the birth test `y *
(t - t0) >= 1.0`, which a negative `y` never satisfies. That is the right place
for it — a rate that cannot be acted on is still the correct control signal,
and suppressing it in the state is the classic windup mistake. The port
matches, and using `max(y, 0)` versus raw `y` in that test is provably
equivalent (both fail for negative `y`) and measured identical to every digit.

**Two things I suspected and measurement refuted**, recorded so they are not
re-suspected: feeding the *clamped* rate back into the controller state — the
windup mistake the reference avoids — changes nothing here, because the clamp
never fires; and spacing births deterministically at `1/y` rather than as a
Poisson process leaves the mean alone while inflating the variance 25×
(var/mean 11.8 instead of 0.48).

## FR-015 — the recorded rationale was RIGHT

Seeding an exact pool of 10 objects with a normal(10000, 1000) lifetime, churn
per 2000-vs window, mean of 40 seeds:

| generation | seeded from lifetime | modulation | seeded from residual life | modulation |
| --- | --- | --- | --- | --- |
| 0 | `[0.0 0.0 0.0 0.3 4.3]` | 1.00 | `[1.7 2.1 1.9 2.4 1.9]` | 0.15 |
| 7 | `[2.8 1.6 1.1 1.8 2.8]` | **0.44** | `[2.1 1.9 2.1 2.0 2.0]` | **0.04** |

Steady state is `N·window/E[L]` = 2.0. Seeded from the lifetime distribution,
the first three windows see *exactly zero* churn — 6000 virtual seconds in
which no key is ever replaced — and the modulation is still 44% peak-to-trough
after eight full lifetimes. Seeded from residual life, churn is flat at 2.0
from `t = 0`.

`residual_sampler` in `seeding.py` is the algorithm FR-015 requires and T020
must implement: a **length-biased** pick of a lifetime, then a uniform point
within it. It needs no closed form, which matters because a lifetime may be
written as any of five distribution kinds including an empirical sample set.

The exponential table is a **control**, not a result: exponential is
memoryless, so its equilibrium residual life is the same exponential and the
two seedings must agree. They do. A difference there would mean the sampler is
wrong.

**Corrected relative to `~scooter/popsim/seed.py`:** its exponential comparison
ran one seed per arm, and its residual sampler consumed 200 000 draws from the
*same* generator before the main loop, so the two arms were different
realisations rather than a controlled comparison. Its means came out 1.96 and
2.32 — about 2.4 standard errors apart, which reads as a real difference but is
noise, and which contradicts the memorylessness it was meant to demonstrate.
Also, it used `np.maximum(1.0, normal(...))`, which clamps where the shipped
implementation truncates (FR-008); immaterial at 10σ, but not the same
operation, so this version truncates by resampling.

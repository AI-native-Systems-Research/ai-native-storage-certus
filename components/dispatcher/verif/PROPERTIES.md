# Verified properties — dispatcher (Creusot)

Proven from spec `specs/001-dispatcher-cache-interface/spec.md` against code
`src/io_segmenter.rs` and `src/lib.rs`. Artifacts: `verif/`.

Formal verification of the dispatcher's pure **arithmetic cores** — MDTS I/O
segmentation and the memory-tier eviction loop's scan/termination math — proved
with [Creusot](https://github.com/creusot-rs/creusot) and discharged by the
`alt-ergo`, `z3`, `cvc5`, and `cvc4` SMT solvers.

- **Crate:** `components/dispatcher/verif/`
- **Status:** `Proved (3 files) ✔` — 44 verification conditions, 0 admits.

## Reproduce

```bash
cd components/dispatcher/verif
cargo creusot --only coma   # fast syntax/translation check (no solvers)
cargo creusot               # full proof — expect: Proved (3 files) ✔
```

## What is a "mirror"

The shipped dispatcher functions cannot compile under Creusot (they touch
`Arc<dyn IMemoryTier>`, `Mutex`, atomics, raw pointers, SPDK/CUDA FFI). Each
proof below runs on a **byte-faithful copy** of a pure arithmetic core, with the
substitutions below. A residual drift gap therefore exists between mirror and
shipped code; line-faithfulness and fault injection keep it honest.

| Shipped code | Verification mirror |
|---|---|
| `assert!(max_transfer_size > 0)` / `assert!(sector_size > 0)` | `#[requires(… > 0)]` (the panic documents the same domain) |
| `Vec::with_capacity(total_bytes.div_ceil(mts))` | `Vec::new()` — capacity is a contractless hint, no bearing on output |
| `remaining.min(mts)` | `if remaining < mts { remaining } else { mts }` (identical value; avoids modelling `Ord::min` on the hot line) |
| `while used()+needed > capacity()` loop over `Arc<dyn IMemoryTier>` | `evict_bound` counter loop on the worst-case guard-always-true path |
| `mt.oldest_keys(scan)` / `dm.try_evict_to_block` / `dm.remove` | not modelled — only the `scan` **window arithmetic** is proved (`scan_widen`) |

## segment_io — MDTS-aware I/O segmentation

Mirror of `src/io_segmenter.rs::segment_io` (lines 22–55).
Spec basis: **FR-019** ("All block device I/O operations MUST be segmented to
respect MDTS"), **SC-012**, and the `io_segmenter` assumption.

- **[Postcondition]** `total_bytes == 0` produces no segments; `total_bytes > 0`
  produces at least one. — spec FR-019 / SC-012 — proved: `segment_io.coma` (33/33 VCs)
- **[Postcondition · MDTS bound]** Every produced segment is at most
  `max_transfer_size` bytes. — spec FR-019 — proved: `segment_io.coma`
- **[Postcondition · positivity]** Every produced segment has a strictly positive
  length (no empty splits). — spec FR-019 — proved: `segment_io.coma`
- **[Postcondition · coverage]** The segments tile `[0, total_bytes)` exactly: the
  first segment starts at buffer offset 0, and the last ends at
  `buffer_offset + length == total_bytes`. — spec FR-019 / SC-012 — proved: `segment_io.coma`
- **[Postcondition · LBA floor]** Every segment's LBA is `≥ start_lba` (no address
  underflow below the transfer's base LBA). — spec US-9 / FR-019 — proved: `segment_io.coma`
- **[Precondition]** `max_transfer_size > 0`, `sector_size > 0` (the two shipped
  `assert!` guards), and `start_lba + total_bytes ≤ u64::MAX` (overflow-freedom of
  the running LBA; the total advance `Σ length/ss ≤ Σ length = total_bytes`).

## scan_widen — eviction scan-window widening

Mirror of `let scan = (MAX_SCAN * attempts).min(1024);` in
`src/lib.rs::evict_for_space` (line 995; `MAX_SCAN = 4` at line 970).
Spec basis: **FR-024** ("scans `IMemoryTier::oldest_keys(scan)` where the scan
widens as pressure persists — `MAX_SCAN = 4 × attempts`, capped at 1024").

- **[Postcondition · bounded]** The scan window never exceeds 1024, regardless of
  attempt count — `evict_for_space` can never request an unbounded LRU scan. —
  spec FR-024 — proved: `scan_widen.coma` (5/5 VCs)
- **[Postcondition · exact-below-cap]** While `4·attempts ≤ 1024`, the window is
  exactly `4·attempts` (it widens by `MAX_SCAN` each attempt). — spec FR-024 — proved: `scan_widen.coma`
- **[Postcondition · floor]** With at least one attempt the window is at least
  `MAX_SCAN` (= 4). — spec FR-024 — proved: `scan_widen.coma`
- **[Precondition]** `attempts ≥ 1`; `MAX_SCAN * attempts ≤ usize::MAX`
  (the caller bounds `attempts` by `max_attempts`, default 2048).

## evict_bound — eviction-loop termination bound

Mirror of the attempt counter in `src/lib.rs::evict_for_space` (lines 972–1006:
`attempts += 1; if attempts > max_attempts { return Err(AllocationFailed) }`).
Spec basis: **FR-024** and **User Story 7, acceptance scenario 5** ("`evict_for_space`
iterates `max_eviction_attempts` times without freeing enough space, then returns
`AllocationFailed`").

- **[Postcondition · termination]** The loop **terminates**, and on the worst-case
  path (no space ever frees, so the `used + needed > capacity` guard stays true) it
  does so after exactly `max_attempts + 1` iterations — it cannot spin unboundedly
  and gives up (rather than blind-free a pinned slot) once the budget is spent. —
  spec FR-024 / US-7 sc.5 — proved: `evict_bound.coma` (6/6 VCs)
- **[Precondition]** `max_attempts < usize::MAX` (so `max_attempts + 1` does not overflow).

## Assumptions / trusted boundaries

- **Mirror, not shipped code.** All three proofs run on standalone mirrors, not
  the shipped functions; injecting a fault into the *shipped* function will not
  fail these proofs. The residual gap is guarded by line-faithfulness to the
  cited source and the fault-injection log below.
- **`evict_bound` proves the counter-driven bound only.** The real
  `evict_for_space` loop also exits *early* when `evict_one_clean` frees enough
  bytes to satisfy `used + needed ≤ capacity`. That early-exit direction depends
  on memory-tier `used()` decreasing monotonically as slots are freed — an
  environment property of `IMemoryTier`/`IDispatchMap` outside this mirror. What
  is proved is the *upper bound*: the loop cannot exceed `max_attempts + 1`
  iterations, i.e. it always terminates and surfaces `AllocationFailed` rather
  than looping forever or blind-freeing a pinned slot.
- **`scan_widen` proves the window arithmetic only.** The container operations it
  feeds (`mt.oldest_keys(scan)`, `dm.try_evict_to_block`, `dm.remove`) are trusted
  boundaries — not modelled here.
- **`segment_io` coverage is stated on endpoints + per-segment length**, which
  together with contiguity of the `buffer_offset` accumulator (loop invariant)
  characterise the tiling. The internal `Vec::with_capacity` hint is dropped in
  the mirror (benign — capacity does not affect the produced segments).

## Fault-injection validation

Each proof was confirmed non-vacuous by perturbing its mirror body and observing
a verification condition go red (`✘`); all three faults were reverted afterward.

| Injected fault | Result |
|---|---|
| `segment_io`: `length = remaining` (drop the MDTS `min(remaining, mts)` cap) | `vc_segment_io` ✘ (32/33) — the MDTS bound is load-bearing |
| `scan_widen`: return `MAX_SCAN * attempts` (drop `.min(1024)`) | `vc_scan_widen` ✘ (4/5) — the 1024 cap is load-bearing |
| `evict_bound`: loop `while attempts <= max_attempts + 1` (one iteration too many) | `vc_evict_bound` ✘ (4/6) — the exact iteration bound is load-bearing |

## Attempted but not proven

- **Quadratic eviction-pressure curve** (FR-048: `multiplier = 1.0 + 7.0 × pressure²`,
  `pressure = (utilization − threshold) / (1 − threshold)`, in
  `background.rs::MemoryTierEvictor`). Out of Creusot scope: it is `f64` arithmetic,
  and Creusot's `@` model targets mathematical integers, not floats/reals.
- **Full `evict_for_space` functional correctness** (that it frees *enough* bytes).
  Depends on `IMemoryTier::used()` monotonicity across `oldest_keys`/`remove`
  interleavings — an environment property, not pure arithmetic. Only the scan-window
  math (`scan_widen`) and the termination bound (`evict_bound`) are proved.
- **`pins.rs::PinnedKeys`** (RAII read-pin release). No arithmetic core to prove;
  its safety property (`release_read` is underflow-safe) is already covered by the
  `dispatch-map` Creusot proof of `release_read`.

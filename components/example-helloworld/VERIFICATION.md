# Kani Pareto-Optimal Verification Campaign: example-helloworld

**Campaign ID**: kani-pareto-example-helloworld  
**Date**: 2026-05-07  
**Target**: `/home/cornel/ai-native-storage-certus/components/example-helloworld/src/lib.rs`  
**Iterations completed**: 1 of 5 (iteration 2 stopped — see §4)

---

## 1. Pareto-Optimal Finding

**`unwind_limit=1` is the Pareto-optimal setting.**

All 4 `GreeterHandler` harnesses pass at the minimum possible unwind bound:

| unwind_limit | success | failing_count | latency_s |
|:---:|:---:|:---:|:---:|
| 1 | true | 0 | 1.364 |
| 2 | true | 0 | 1.363 |
| 3 | true | 0 | 1.372 |
| 4 | true | 0 | 1.365 |
| 5 | true | 0 | 1.362 |
| 10 | true | 0 | 1.367 |

**Pareto frontier is degenerate**: correctness is constant (all pass) and latency is constant (~1.36 s) across the entire sweep. The minimum correct value — `unwind_limit=1` — is therefore Pareto-optimal by definition.

---

## 2. Harnesses Verified

All 4 harnesses in `#[cfg(kani)] mod verification` in `src/lib.rs`:

| Harness | Target | Checks | Status |
|---|---|:---:|:---:|
| `verify_greeter_new_initial_state` | `GreeterHandler::new()` postcondition | — | PASS |
| `verify_count_increment_bounded` | `count+1` correctness (with `assume`) | — | PASS |
| `verify_count_two_step_increment` | Two-step monotonicity (with `assume`) | — | PASS |
| `verify_greeter_default_equals_new` | `default() == new()` state equality | — | PASS |

**Total checks at unwind=1**: 213 (0 failed, 2 unreachable).

---

## 3. Extracted Principles

### RP-1 — Minimum Unwind Threshold (confidence: high)
For the 4 loop-free harnesses in example-helloworld, `unwind_limit=1` is both necessary and sufficient. All 213 checks pass; `failing_count=0`.

**Mechanism**: All harnesses are explicitly loop-free. CBMC requires only a single unroll step to cover all reachable paths. No additional path expansion occurs at higher bounds.

**Applicability bounds**: Does not hold if loop-containing harnesses are added, or if iterator chains are introduced into the functions under test.

---

### RP-2 — Flat Latency Curve (confidence: high)
Verification latency is flat across `unwind_limit` values 1–10 (~1.36 s per run on a warm build cache).

**Mechanism**: With no loops, the CBMC formula size is invariant to the unwind bound. The CBMC solver itself takes ~0.063 s; the remaining ~1.3 s is fixed Cargo incremental-compilation overhead.

**Applicability bounds**: Latency will scale with the unwind bound if loop-containing harnesses are introduced.

---

### RP-3 — Latent Overflow Defect (confidence: high)
`verify_count_increment_bounded` passes at all tested unwind values because `kani::assume(init < u32::MAX)` precludes the overflow path. However, **the production code contains a latent unchecked overflow defect**:

```rust
// src/lib.rs — ActorHandler::handle()
fn handle(&mut self, msg: GreetRequest) {
    self.count += 1;  // bare increment — no overflow guard in production
    ...
}
```

The `kani::assume` in the harness is **unmatched**: there is no corresponding guard in production. A caller that delivers `u32::MAX` messages would trigger wrapping (debug: panic; release: silent wrap). The harness result is sound within its assumed domain but does not cover the full `u32` input range.

**Recommended fix**: Replace bare `+= 1` with `self.count = self.count.saturating_add(1)` and update the harness to drop the `assume`, then re-verify.

---

### MP-1 — Cold vs Warm Build Cache (confidence: high)
The documented anchor latency (5.42 s) reflected a cold-build cost. All subsequent evaluator calls on a warm build cache run in ~1.36 s (~74% faster). Predictions anchored to the first-run value will be off by ~4× for warm-cache runs.

**Mechanism**: Cargo's incremental compilation caches compiled artefacts after the first build. The CBMC solver itself is fast (~0.063 s); the dominant cost is the Cargo build pipeline.

---

### MP-2 — Unwind is Non-Informative for Loop-Free Suites (confidence: high)
For loop-free Kani harness suites, sweeping `unwind_limit` from 1 to 10 yields identical correctness outcomes and statistically indistinguishable latencies. Future campaigns should not expend experimental budget on unwind sweeps for these harnesses unless new loop-containing harnesses are added.

---

## 4. Iteration 2 — Stopped (max redesigns reached)

**Goal of iteration 2**: Test the latent overflow defect (RP-3) by removing the `kani::assume` and verifying that the unguarded `+= 1` fails under full `u32` range.

**Why it failed**: The Nous framework's LLM dispatcher (`repo_path: null`) designed experiments using `git apply patches/h-main.patch`. Because `repo_path: null` means no CLI dispatcher (no shell access), the patch files were never created on disk. All 3 redesign attempts failed with `exit 128` on `git apply`.

**Root cause**: Patch-based code modifications require `repo_path` set (activates `CLIDispatcher` with real shell access). With `repo_path: null`, the LLM can only generate evaluator calls — it cannot write files.

**Recommended fix for future campaign**: Set `repo_path` in `kani_campaign.yaml`, or pre-create a variant harness file and point the evaluator at it.

---

## 5. Campaign Metadata

| Metric | Value |
|---|---|
| Campaign framework | Nous (agentic-strategy-evolution) |
| LLM model | aws/claude-sonnet-4-6 |
| Total LLM calls | 20 |
| Total input tokens | 125,431 |
| Total output tokens | 33,261 |
| Total LLM duration | 522.7 s |
| Output directory | `agentic-strategy-evolution/kani-pareto-example-helloworld/` |

---

## 6. Files Changed

| File | Change |
|---|---|
| `components/example-helloworld/src/lib.rs` | Added `#[cfg(kani)] mod verification` with 4 harnesses |
| `agentic-strategy-evolution/kani_evaluator.py` | Evaluator bridge pointing at example-helloworld |
| `agentic-strategy-evolution/kani_campaign.yaml` | Campaign definition for Pareto unwind sweep |

---

## 7. Recommended Next Actions

1. **Fix the overflow defect**: Change `self.count += 1` to `self.count = self.count.saturating_add(1)` in `GreeterHandler::handle()`.

2. **Add an overflow harness without assume**:
   ```rust
   #[kani::proof]
   #[kani::unwind(1)]
   fn verify_count_no_overflow() {
       let mut h = GreeterHandler::new();
       h.count = kani::any();
       // With saturating_add this should succeed for all inputs
       let before = h.count;
       h.count = h.count.saturating_add(1);
       kani::assert(h.count >= before, "saturating increment must not decrease count");
   }
   ```

3. **Re-run dry run** to confirm the fix: `python kani_evaluator.py '{"unwind_limit": 1}'`

4. **For future Nous campaigns with code modification experiments**: Set `repo_path` in `kani_campaign.yaml` so the CLIDispatcher is used and the LLM can write patch files to disk.

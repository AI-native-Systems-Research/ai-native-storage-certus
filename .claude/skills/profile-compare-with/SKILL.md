---
name: profile-compare-with
description: Compare performance of the current branch against another branch (default unstable)
argument-hint: "[branch-name]"
---

Compare the performance of the current branch against a reference branch using `certus-api-bench_v2.py`. The benchmark runs on the local node with identical parameters for both branches, producing a side-by-side comparison.

## Arguments

- `branch-name` (optional): The branch to compare against. Defaults to `unstable`.

## Workflow

### 1. Pre-flight

- Record the current branch name: `CURRENT_BRANCH=$(git branch --show-current)`
- Record the comparison branch: `COMPARE_BRANCH` (from argument, default `unstable`)
- Check for uncommitted changes. If any exist, auto-commit them:
  ```bash
  git add -A && git commit -m "wip: auto-save before performance comparison"
  ```
- Verify both branches exist locally (fetch if needed):
  ```bash
  git fetch origin $COMPARE_BRANCH 2>/dev/null || true
  ```

### 2. Build and Benchmark Current Branch

- Build release:
  ```bash
  CERTUS_PROFILE=full cargo build -p certus-server-yaml --release
  ```
- Start the server:
  ```bash
  target/release/certus-server-yaml --device-path /dev/null --format &
  SERVER_PID=$!
  sleep 5
  ```
- Run the benchmark:
  ```bash
  cd apps/python
  python3 certus-api-bench_v2.py \
      --clients 1 \
      --num-objects 32 \
      --iterations 20 \
      --block-size 4M \
      --batch-size 16 \
      --pipeline-depth 4
  ```
- Capture the full output (including throughput numbers) to `CURRENT_RESULTS`
- Stop the server:
  ```bash
  kill $SERVER_PID && wait $SERVER_PID 2>/dev/null
  ```

### 3. Switch to Comparison Branch, Build and Benchmark

- Checkout the comparison branch:
  ```bash
  git checkout $COMPARE_BRANCH
  ```
- Build release (same profile):
  ```bash
  CERTUS_PROFILE=full cargo build -p certus-server-yaml --release
  ```
- Start the server (same flags):
  ```bash
  target/release/certus-server-yaml --device-path /dev/null --format &
  SERVER_PID=$!
  sleep 5
  ```
- Run the same benchmark with identical parameters
- Capture output to `COMPARE_RESULTS`
- Stop the server

### 4. Return to Original Branch

```bash
git checkout $CURRENT_BRANCH
```

### 5. Report

Print a comparison table with the key metrics extracted from both runs:

```
================================================================
Performance Comparison: $CURRENT_BRANCH vs $COMPARE_BRANCH
================================================================

Benchmark: certus-api-bench_v2.py
Parameters: --clients 1 --num-objects 32 --iterations 20 --block-size 4M --batch-size 16 --pipeline-depth 4

                          Current ($CURRENT_BRANCH)    Reference ($COMPARE_BRANCH)    Delta
Populate throughput:      X.XX GB/s                    Y.YY GB/s                      +/-Z.Z%
Hot lookup throughput:    X.XX GB/s                    Y.YY GB/s                      +/-Z.Z%
Cold lookup throughput:   X.XX GB/s                    Y.YY GB/s                      +/-Z.Z%
Hot lookup latency (p50): X.XX ms                     Y.YY ms                        +/-Z.Z%
Hot lookup latency (p99): X.XX ms                     Y.YY ms                        +/-Z.Z%
```

Mark regressions (>5% slower) with ⚠️ and improvements (>5% faster) with ✓.

## Important Notes

- The server is started with `--device-path /dev/null` (no real SSD) so only hot-path (memory-tier) performance is measured. Cold-path results will show as N/A or zero.
- If the comparison branch does not compile, report the build error and skip its benchmark.
- Always return to the original branch at the end, even if something fails.
- The benchmark parameters above are defaults — the user may override them in their invocation message.
- Both builds use the SAME profile (`CERTUS_PROFILE=full`) to ensure apples-to-apples comparison. If the current branch requires a different profile (e.g., `full-remote`), note this in the output.

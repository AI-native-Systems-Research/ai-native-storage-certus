# Certus Codebase Summary

Generated: 2026-05-22

## Total Lines by Language

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Language              Files        Lines         Code     Comments       Blanks
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Python                  965       281915       233736        15639        32540
 C++ Header               16        51642        36411         8828         6403
 Rust                    194        44777        36746         2055         5976
 C++                      39        23785        18471         1496         3818
 JSON                    641        14710        14701            0            9
 YAML                    132         9563         7475          963         1125
 C                         8         3901         2849          516          536
 Shell                    78         4090         2776          644          670
 JavaScript               10         2754         2427          176          151
 CSS                       3         1243         1164           22           57
 TOML                     33          898          768           11          119
 CUDA                      1          796          647           44          105
 Other                   484        16628         8944         5782         1897
 Markdown                275        81135            0        49201        31934
 Plain Text              132        16447            0        14965         1482
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                  2604       556408       366935       101576        87897
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## Rust Component Breakdown

| Component | Code | Tests | Total |
|-----------|------|-------|-------|
| tools/creusot (vendored) | 64,214 | 729 | 64,943 |
| component-framework/crates | 5,013 | 3,875 | 8,888 |
| gpu-services/src | 1,990 | 192 | 2,182 |
| block-device-spdk-nvme/src | 1,870 | 472 | 2,342 |
| extent-manager/src | 1,778 | 355 | 2,133 |
| dispatcher/src | 1,541 | 1,350 | 2,891 |
| dispatcher/benches | 1,632 | 0 | 1,632 |
| interfaces/src | 1,032 | 146 | 1,178 |
| apps/iops-benchmark-md/src | 1,219 | 314 | 1,533 |
| apps/iops-benchmark/src | 929 | 298 | 1,227 |
| apps/extent-benchmark/src | 629 | 0 | 629 |
| apps/gpu-bb-vs-p2p/src | 579 | 0 | 579 |
| apps/certus-server/src | 551 | 0 | 551 |
| certus-connector/src/engine.rs | 535 | 0 | 535 |
| dispatch-map/src | 456 | 324 | 780 |
| gpu-services/benches | 418 | 0 | 418 |
| memory-tier/src | 365 | 312 | 677 |
| spdk-env/src | 349 | 607 | 956 |
| apps/nvme-ns-manager/src | 315 | 0 | 315 |
| logger/src | 135 | 170 | 305 |

## Complexity Metrics

### Top 15 Largest Source Files (non-test, project code)

| File | Lines |
|------|-------|
| components/dispatcher/src/lib.rs | 2,609 |
| components/component-framework/crates/component-core/src/actor.rs | 1,261 |
| components/block-device-spdk-nvme/src/actor.rs | 1,164 |
| components/gpu-services/src/lib.rs | 1,083 |

### Top Files by Function/Method Count (project code)

| File | Fns |
|------|-----|
| components/dispatcher/src/lib.rs | 123 |
| components/component-framework/crates/component-core/src/actor.rs | 61 |
| components/dispatcher/tests/lazy_migration.rs | 60 |
| components/dispatcher/benches/dispatcher_benchmark.rs | 60 |

### Unsafe Usage

- Files containing unsafe: **100**
- Total unsafe occurrences: **690**

### Deepest Nesting (project code, top 5)

| File | Depth |
|------|-------|
| components/extent-manager/src/lib.rs | 9 |
| components/dispatcher/src/lib.rs | 9 |
| components/component-framework/crates/component-core/src/actor.rs | 9 |

## Key Ratios

- **Code : Tests** — ~60:40 in core components (strong test coverage)
- **Comments** — 5.3% of Rust lines (lean, consistent with project style)
- **Unsafe** — 690 occurrences across 100 files (concentrated in SPDK/CUDA FFI layers)
- **Total code across all languages**: 366,935 lines

## Complexity Hotspots (project code only, excluding vendored tools/creusot)

| Metric | File | Value |
|--------|------|-------|
| Largest file | dispatcher/src/lib.rs | 2,609 lines |
| Most functions | dispatcher/src/lib.rs | 123 fns |
| Deepest nesting | extent-manager/src/lib.rs | depth 9 |
| Deepest nesting | dispatcher/src/lib.rs | depth 9 |

# Certus Codebase Summary

Generated: 2026-05-05

## Total Lines by Language

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Language              Files        Lines         Code     Comments       Blanks
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 JSON                      4         7010         7010            0            0
 Python                   14         2397         1872          115          410
 Shell                    15         1201          920          142          139
 SVG                       4          858          622          151           85
 TOML                     29          668          563           11           94
 Protocol Buffers          2          204          130           26           48
 PlantUML                  1          130           93           10           27
 C Header                  1            4            3            1            0
 Plain Text               13        14305            0        13108         1197
─────────────────────────────────────────────────────────────────────────────────
 Rust                    177        37924        31277         1468         5179
 |- Markdown             139         6164           53         4743         1368
 (Total)                            44088        31330         6211         6547
─────────────────────────────────────────────────────────────────────────────────
 Markdown                209        53256            0        32721        20535
 |- BASH                  61          884          675          135           74
 |- JSON                   4           18           18            0            0
 |- Markdown               3          150            0          108           42
 |- Python                 3           55           43            4            8
 |- Rust                  36         2431         1786          346          299
 |- TOML                   3           74           67            0            7
 (Total)                            56868         2589        33314        20965
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                   469       127733        45132        53089        29512
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## Complexity Metrics

### Lines of Rust Code per Component

| Component | Code | Tests | Total |
|-----------|------|-------|-------|
| tools/creusot/creusot (vendored) | 64,214 | 729 | 64,943 |
| components/component-framework/crates | 5,013 | 3,875 | 8,888 |
| components/block-device-spdk-nvme/v2 | 2,216 | 1,326 | 3,542 |
| components/block-device-spdk-nvme/v1 | 2,045 | 821 | 2,866 |
| components/extent-manager/v2 | 1,957 | 1,016 | 2,973 |
| apps/iops-benchmark-md/src | 1,238 | 321 | 1,559 |
| components/dispatcher/v0 | 996 | 1,315 | 2,311 |
| apps/iops-benchmark/src | 948 | 309 | 1,257 |
| components/component-framework/examples | 944 | 0 | 944 |
| components/gpu-services/v0 | 925 | 131 | 1,056 |
| components/interfaces/src | 905 | 127 | 1,032 |
| apps/extent-benchmark/src | 629 | 0 | 629 |
| components/dispatch-map/v0 | 446 | 568 | 1,014 |
| apps/certus-server/src | 444 | 0 | 444 |
| components/spdk-env/src | 349 | 607 | 956 |
| apps/nvme-ns-manager/src | 329 | 0 | 329 |
| certus-connector/src/engine.rs | 284 | 0 | 284 |
| components/logger/v1 | 221 | 260 | 481 |
| components/spdk-sys/build.rs | 197 | 0 | 197 |
| apps/gpu-handle-test-server/src | 76 | 0 | 76 |
| components/example-helloworld/src | 74 | 0 | 74 |
| certus-connector/src/lib.rs | 60 | 0 | 60 |
| apps/certus-server/build.rs | 60 | 0 | 60 |
| apps/gpu-show/src | 50 | 0 | 50 |
| tools/creusot/creusot-test-example | 40 | 0 | 40 |
| components/spdk-env/examples | 40 | 0 | 40 |
| apps/helloworld-mainline/src | 26 | 0 | 26 |
| certus-connector/src/keys.rs | 9 | 0 | 9 |
| components/spdk-sys/src | 6 | 0 | 6 |
| certus-connector/build.rs | 3 | 0 | 3 |
| components/spdk-sys/tests | 1 | 76 | 77 |

### Top 15 Largest Source Files (non-test, by code lines)

| File | Lines |
|------|-------|
| tools/creusot/creusot/pearlite-syn/src/term.rs | 2,307 |
| tools/creusot/creusot/creusot/src/validate/erasure.rs | 1,897 |
| tools/creusot/creusot/creusot/src/backend/program.rs | 1,463 |
| tools/creusot/creusot/why3/src/printer.rs | 1,362 |
| tools/creusot/creusot/creusot/src/backend/clone_map/elaborator.rs | 1,282 |
| components/component-framework/crates/component-core/src/actor.rs | 1,261 |
| components/block-device-spdk-nvme/v2/tests/integration.rs | 1,245 |
| components/dispatcher/v0/src/lib.rs | 1,230 |
| components/block-device-spdk-nvme/v2/src/actor.rs | 1,164 |
| tools/creusot/creusot/creusot-std/src/logic/seq.rs | 1,076 |
| tools/creusot/creusot/creusot/src/analysis.rs | 1,064 |
| components/block-device-spdk-nvme/v1/src/actor.rs | 1,043 |
| tools/creusot/creusot/creusot/src/translation/pearlite/from_thir.rs | 977 |
| tools/creusot/creusot/creusot-std-proc/src/creusot/extern_spec.rs | 965 |
| tools/creusot/creusot/creusot/src/translation/pearlite.rs | 961 |

### Top 15 Files by Function/Method Count

| File | Fns |
|------|-----|
| tools/creusot/creusot/pearlite-syn/src/term.rs | 101 |
| tools/creusot/creusot/creusot-std/src/std/ops.rs | 99 |
| tools/creusot/creusot/creusot/src/validate/erasure.rs | 91 |
| tools/creusot/creusot/creusot-std/src/std/ptr.rs | 85 |
| components/dispatcher/v0/src/lib.rs | 78 |
| tools/creusot/creusot/creusot-std/src/std/slice.rs | 77 |
| tools/creusot/creusot/creusot-std/src/logic/seq.rs | 74 |
| tools/creusot/creusot/creusot-std/src/std/option.rs | 73 |
| tools/creusot/creusot/why3/src/exp.rs | 61 |
| components/component-framework/crates/component-core/src/actor.rs | 61 |
| tools/creusot/creusot/why3/src/printer.rs | 60 |
| tools/creusot/creusot/creusot-std/src/logic/int.rs | 60 |
| tools/creusot/creusot/examples/red_black_tree.rs | 53 |
| tools/creusot/creusot/creusot-std/src/logic/fmap.rs | 52 |
| tools/creusot/creusot/creusot/src/backend/clone_map.rs | 52 |

### Unsafe Usage

- Files containing unsafe: **94**
- Total unsafe occurrences: **477**

### Deepest Nesting (max brace depth per file, top 10)

| File | Depth |
|------|-------|
| tools/creusot/creusot/creusot/src/translation/function/statement.rs | 11 |
| tools/creusot/creusot/why3/src/exp.rs | 10 |
| tools/creusot/creusot/pearlite-syn/tests/test_term.rs | 10 |
| tools/creusot/creusot/creusot/src/translation/pearlite/from_thir.rs | 10 |
| tools/creusot/creusot/pearlite-syn/src/term.rs | 9 |
| tools/creusot/creusot/creusot/src/validate/purity.rs | 9 |
| tools/creusot/creusot/creusot/src/validate/opacity.rs | 9 |
| tools/creusot/creusot/creusot/src/backend/program.rs | 9 |
| components/extent-manager/v2/src/lib.rs | 9 |
| components/component-framework/crates/component-core/src/actor.rs | 9 |

## Key Ratios

- **Code : Tests** — ~60:40 in core components (strong test coverage)
- **Comments** — 4.7% of Rust lines (lean, consistent with project style)
- **Unsafe** — 477 occurrences across 94 files (concentrated in SPDK FFI layers)

## Complexity Hotspots (project code only, excluding vendored tools/creusot)

| Metric | File | Value |
|--------|------|-------|
| Largest file | component-core/src/actor.rs | 1,261 lines |
| Most functions | dispatcher/v0/src/lib.rs | 78 fns |
| Deepest nesting | extent-manager/v2/src/lib.rs | depth 9 |
| Deepest nesting | component-core/src/actor.rs | depth 9 |

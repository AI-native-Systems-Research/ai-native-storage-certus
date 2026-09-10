# Codebase SLOC & Complexity Summary

**Scope:** repository root (`/home/dwaddington/ai-native-storage-certus`)
**Generated:** 2026-09-10 · tokei + `.claude/skills/tools-count-sloc/complexity.sh`
**Excluded from source counts:** `target/` (build artifacts), `deps/spdk-build/`
(vendored SPDK), `tools/creusot/` (vendored formal-verification submodule).

---

## Headline: Certus-authored source

| Language | Files | Code | Comments | Blanks |
|---|---:|---:|---:|---:|
| **Rust** (core + components + apps) | 300 | **76,787** | 4,437 | 11,396 |
| **Python** (benchmark drivers, tooling) | 194 | **48,684** | 3,450 | 7,487 |
| Shell | 121 | 8,631 | 3,297 | 1,281 |
| YAML | 78 | 6,505 | 255 | 285 |
| TOML | 53 | 1,101 | 118 | 172 |

Rust is the primary implementation language; Python is almost entirely the
`benchmarks/` replay drivers and analysis tooling.

> `tokei` respects `.gitignore` and does not descend into submodules, so the
> Rust figure already **excludes** the vendored Creusot tool (~66k lines under
> `tools/creusot/`). The complexity script below uses raw `find` and therefore
> *does* include Creusot — its rankings are annotated accordingly.

## What the raw repo-root totals contain (context)

The unfiltered `tokei .` total is ~4.0M lines, dominated by **non-source**
material that should not be read as codebase size:

- **JSON — 3,267,127 lines** (72 files): benchmark corpora / results / traces.
- **C / C Header / Cython — ~450k lines**: vendored SPDK + generated FFI.
- **Markdown — 147k comment-lines**: docs, transcripts, knowledge wiki.

These are data/vendored/generated, not Certus implementation.

---

## Rust by component (top, non-test code lines; Creusot excluded)

| Component | Code | Tests | Total |
|---|---:|---:|---:|
| `lib/component-framework/crates` | 5,044 | 3,928 | 8,972 |
| `components/dispatcher/src` | 3,805 | 2,879 | 6,684 |
| `components/dispatcher-p2p/src` | 3,625 | 2,611 | 6,236 |
| `components/remote-lookup/src` | 2,436 | 354 | 2,790 |
| `components/block-device-spdk-nvme/src` | 2,177 | 477 | 2,654 |
| `components/gpu-services/src` | 2,169 | 195 | 2,364 |
| `components/remote-lookup-rdma-initiator/src` | 2,102 | 955 | 3,057 |
| `components/extent-manager/src` | 1,927 | 387 | 2,314 |
| `components/interfaces/src` | 1,815 | 361 | 2,176 |
| `components/remote-lookup-rdma-responder/src` | 1,415 | 549 | 1,964 |

*(Vendored `tools/creusot/creusot` reports 66,177 code lines — a formal-verification
tool, not Certus source; omitted from the table.)*

## Largest source files (non-test, Creusot & backups excluded)

| File | Code lines |
|---|---:|
| `components/dispatcher/src/lib.rs` | 6,381 |
| `components/dispatcher-p2p/src/lib.rs` | 4,927 |
| `components/remote-lookup-rdma-initiator/src/connection.rs` | 2,128 |
| `components/block-device-spdk-nvme/src/actor.rs` | 1,500 |
| `components/dispatcher-p2p/src/pipeline.rs` | 1,402 |
| `components/gpu-services/src/lib.rs` | 1,318 |
| `lib/component-framework/crates/component-core/src/actor.rs` | 1,273 |
| `apps/remote-lookup-bench/src/main.rs` | 1,265 |

## Highest function/method count (Creusot excluded)

| File | Fns |
|---|---:|
| `components/dispatcher/src/lib.rs` | 211 |
| `components/dispatcher-p2p/src/lib.rs` | 181 |
| `components/remote-lookup/src/seams.rs` | 105 |
| `components/remote-lookup-rdma-initiator/src/connection.rs` | 84 |
| `lib/component-framework/crates/component-core/src/actor.rs` | 62 |

## Deepest brace nesting (Creusot excluded)

| File | Depth |
|---|---:|
| `apps/certus-server-yaml/build.rs` | 16 *(generated bindgen build script)* |
| `components/dispatcher-p2p/src/lib.rs` | 12 |
| `components/dispatcher/src/pipeline.rs` | 10 |
| `components/dispatcher/src/lib.rs` | 10 |

## Unsafe usage (whole tree, `find`-based — includes vendored)

- Files containing `unsafe`: **144**
- Total `unsafe` occurrences: **1,212**

Concentrated as expected in SPDK/FFI (`block-device-spdk-nvme`, `spdk-sys`),
RDMA (`remote-lookup-rdma-*`), and GPU handle plumbing (`gpu-services`).

---

## Code vs tests vs comments

- **Rust comment ratio:** 4,437 / 76,787 ≈ **5.8%** of code lines are comments.
- **Test weight:** the `dispatcher` and `component-framework` crates carry the
  most tests (≈2.9k and ≈3.9k test lines respectively) — roughly test-to-code
  parity, the highest in the tree. Many `apps/*` and `benches/*` carry none.
- **Python comment ratio:** 3,450 / 48,684 ≈ **7.1%**.

## Observations

- **`dispatcher/src/lib.rs` is the complexity hotspot** on every axis: largest
  file (6,381 lines), most functions (211), and deep nesting (10). Its
  data-parallel sibling `dispatcher-p2p/src/lib.rs` is a close second. These two
  files alone are ~11k lines — a natural refactor / split candidate.
- The `component-framework` core is well-tested (near 1:1 test:code); the
  dispatchers are moderately tested; RDMA and SPDK components lean lighter.
- Reported "size" of the repo is misleading at face value — 80%+ of raw lines
  are JSON benchmark data and vendored C/Creusot, not authored Certus code.

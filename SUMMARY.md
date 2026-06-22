# Certus Codebase Summary

Generated: 2026-06-18

## Overall SLOC by Language

| Language | Files | Code | Comments | Blanks | Total Lines |
|----------|-------|------|----------|--------|-------------|
| **Rust** | 228 | 52,832 | 2,508 | 8,276 | 63,616 |
| Python | 41 | 9,799 | 538 | 1,659 | 11,996 |
| JSON | 10 | 7,398 | 0 | 0 | 7,398 |
| C | 10 | 2,980 | 522 | 553 | 4,055 |
| C++ | 6 | 2,886 | 493 | 729 | 4,108 |
| Shell | 49 | 2,680 | 585 | 494 | 3,759 |
| YAML | 24 | 1,179 | 52 | 147 | 1,378 |
| CUDA | 3 | 817 | 46 | 123 | 986 |
| TOML | 40 | 834 | 12 | 131 | 977 |
| Markdown (docs) | 269 | — | 80,176 | 59,752 | 139,928 |
| **Total** | **727** | **87,678** | **105,304** | **75,823** | **268,805** |

## Rust Breakdown (Primary Language)

| Metric | Value |
|--------|-------|
| Total Rust code lines | 52,832 |
| Doc comments (in Rust) | 5,822 |
| Inline comments | 2,508 |
| Test code (est.) | ~16,300 |
| Production code (est.) | ~36,500 |
| Test-to-code ratio | ~0.45 |
| Comment-to-code ratio | ~0.16 |

## Component Breakdown (Rust, by code lines)

| Component | Code | Tests | Total | Test % |
|-----------|------|-------|-------|--------|
| component-framework | 5,034 | 3,875 | 8,909 | 43% |
| dispatcher-p2p | 2,781 | 1,917 | 4,698 | 41% |
| dispatcher | 2,471 | 1,936 | 4,407 | 44% |
| gpu-services | 1,989 | 192 | 2,181 | 9% |
| block-device-spdk-nvme | 1,972 | 475 | 2,447 | 19% |
| extent-manager | 1,828 | 359 | 2,187 | 16% |
| block-device-filesys | 1,181 | 96 | 1,277 | 8% |
| block-device-kernel | 1,162 | 91 | 1,253 | 7% |
| interfaces | 1,062 | 145 | 1,207 | 12% |
| eviction-policy-lru | 520 | 310 | 830 | 37% |
| dispatch-map | 503 | 324 | 827 | 39% |
| memory-tier | 496 | 286 | 782 | 37% |
| spdk-env | 356 | 607 | 963 | 63% |
| remote-lookup | 180 | 120 | 300 | 40% |
| logger | 135 | 170 | 305 | 56% |

## Applications (Rust)

| App | Code | Tests | Total |
|-----|------|-------|-------|
| iops-benchmark-md | 1,221 | 314 | 1,535 |
| certus-server | 953 | 0 | 953 |
| iops-benchmark | 931 | 298 | 1,229 |
| certus-server-yaml | 724 | 0 | 724 |
| extent-benchmark | 630 | 0 | 630 |
| gpu-bb-vs-p2p | 581 | 0 | 581 |
| nvme-bar1-bench | 502 | 0 | 502 |
| baseline-generalized-fs | 420 | 0 | 420 |
| nvme-ns-manager | 315 | 0 | 315 |

## Benchmarks (Rust, dedicated bench files)

| Component | Bench Lines |
|-----------|-------------|
| dispatcher-p2p | 1,730 |
| dispatcher | 1,710 |
| gpu-services | 418 |
| block-device-spdk-nvme | 350 |
| block-device-filesys | 193 |
| block-device-kernel | 176 |
| extent-manager | 110 |
| dispatch-map | 108 |
| logger | 69 |

## Complexity Indicators

### Largest Source Files

| File | Lines |
|------|-------|
| `components/dispatcher/src/lib.rs` | 3,992 |
| `components/dispatcher-p2p/src/lib.rs` | 3,747 |
| `components/block-device-spdk-nvme/src/actor.rs` | 1,286 |
| `components/component-framework/.../actor.rs` | 1,273 |
| `components/dispatcher-p2p/src/pipeline.rs` | 1,155 |
| `components/gpu-services/src/lib.rs` | 1,084 |

### Highest Function Count

| File | Functions |
|------|-----------|
| `components/dispatcher/src/lib.rs` | 142 |
| `components/dispatcher-p2p/src/lib.rs` | 137 |
| `components/component-framework/.../actor.rs` | 62 |

### Deepest Nesting

| File | Max Depth |
|------|-----------|
| `apps/certus-server-yaml/build.rs` | 16 |
| `components/dispatcher/src/lib.rs` | 14 |
| `components/dispatcher-p2p/src/lib.rs` | 12 |

### Unsafe Usage

- **125 files** contain `unsafe` blocks
- **989 total** `unsafe` occurrences
- Concentrated in: SPDK FFI bindings, io_uring operations, CUDA interop, DMA buffer management

## Key Ratios

| Ratio | Value | Assessment |
|-------|-------|------------|
| Test / Production code | 0.45 | Good coverage |
| Comments / Code | 0.16 | Concise, relies on naming |
| Docs (markdown) / Code | 1.6x | Heavily documented |
| Unsafe density | 989 / 52,832 = 1.9% | Reasonable for systems code |

# Certus Codebase Summary

Generated: 2026-07-23 (previous run: 2026-06-18)
Scope: repository root — excludes `deps/spdk-build` and `target`.

> **Note on Rust totals:** `tools/creusot/creusot` (the vendored Creusot verifier)
> is **gitignored**, so tokei already excludes it. The 70,781 Rust figure below is
> effectively first-party plus `tools/rdma-test` (~1,807 lines). Scoped to
> first-party only (excluding all of `tools/`), Rust is **68,974 code lines** across
> 272 files — see the dedicated section below.
> (The per-component `complexity.sh` script uses raw `find`, which does NOT respect
> gitignore, so it reports Creusot's ~66k lines — ignore that row as vendored.)

## Overall SLOC by Language (top)

| Language | Files | Code | Comments | Blanks |
|----------|-------|------|----------|--------|
| C | 290 | 149,644 | 14,115 | 29,470 |
| **Rust** | 286 | 70,781 | 3,643 | 10,748 |
| C Header | 243 | 50,351 | 13,627 | 9,972 |
| Python | 155 | 36,889 | 1,657 | 5,590 |
| Cython | 83 | 14,472 | 265 | 2,322 |
| JSON | 59 | 8,009 | 0 | 0 |
| Shell | 78 | 4,581 | 1,107 | 842 |
| CMake | 72 | 3,381 | 424 | 421 |
| C++ | 6 | 2,886 | 493 | 729 |
| YAML | 29 | 1,755 | 84 | 211 |
| TOML | 48 | 1,025 | 60 | 159 |
| Protocol Buffers | 8 | 797 | 217 | 325 |
| CUDA | 3 | 817 | 46 | 123 |
| Markdown (docs) | 651 | — | 125,711 | 84,817 |
| **Total** | **2,196** | **363,980** | **185,551** | **152,612** |

The large C / C-Header / Python / Cython counts are dominated by **vendored
dependencies** (SPDK sources, Creusot tooling), not first-party Certus code.

## Component Breakdown (Rust, by code lines)

| Component | Code | Tests | Total |
|-----------|------|-------|-------|
| component-framework/crates | 5,034 | 3,875 | 8,909 |
| dispatcher-p2p | 3,512 | 2,226 | 5,738 |
| dispatcher | 3,304 | 2,308 | 5,612 |
| remote-lookup | 2,219 | 214 | 2,433 |
| gpu-services | 2,154 | 195 | 2,349 |
| block-device-spdk-nvme | 2,061 | 475 | 2,536 |
| extent-manager | 1,914 | 387 | 2,301 |
| interfaces | 1,741 | 361 | 2,102 |
| remote-lookup-rdma-initiator | 1,663 | 447 | 2,110 |
| remote-lookup-rdma-responder | 1,329 | 511 | 1,840 |
| block-device-filesys | 1,215 | 96 | 1,311 |
| block-device-kernel | 1,193 | 91 | 1,284 |
| extended-metadata-store | 1,062 | 161 | 1,223 |
| memory-tier | 595 | 195 | 790 |
| disk-partition-manager | 582 | 0 | 582 |
| dispatch-map | 562 | 287 | 849 |
| block-device-memory | 506 | 149 | 655 |
| zyre | 473 | 57 | 530 |
| spdk-env | 356 | 607 | 963 |
| eviction-policy-lru | 271 | 270 | 541 |
| logger | 135 | 170 | 305 |

_The above table uses `complexity.sh`'s src/tests split. The authoritative
first-party totals (tokei, excluding all of `tools/`) are below._

## First-Party Rust Only (tokei, excludes `tools/`, `deps/`, `target/`)

Files: 272 · Code: **68,974** · Comments: 3,570 · Blanks: 10,476 ·
Embedded doc-Markdown: 10,322
By area: components 58,789 · apps 9,534 · certus-connector 651

| Component / App | Code | Comments |
|---|---|---|
| component-framework | 9,853 | 445 |
| dispatcher | 9,452 | 694 |
| dispatcher-p2p | 6,902 | 341 |
| block-device-spdk-nvme | 3,826 | 243 |
| gpu-services | 3,338 | 241 |
| extent-manager | 3,257 | 93 |
| remote-lookup | 3,114 | 241 |
| remote-lookup-rdma-initiator | 2,387 | 135 |
| extended-metadata-store | 2,172 | 167 |
| interfaces | 2,101 | 154 |
| block-device-filesys | 1,986 | 47 |
| certus-server-yaml | 1,904 | 74 |
| remote-lookup-rdma-responder | 1,898 | 155 |
| certus-server | 1,889 | 27 |
| block-device-kernel | 1,865 | 38 |
| iops-benchmark-md | 1,535 | 33 |
| iops-benchmark | 1,397 | 45 |
| dispatch-map | 1,341 | 49 |
| spdk-env | 1,003 | 50 |
| zyre | 884 | 54 |
| memory-tier | 790 | 12 |
| extent-benchmark | 719 | 22 |
| block-device-memory | 655 | 3 |
| certus-connector | 651 | 28 |
| disk-partition-manager | 582 | 29 |
| gpu-bb-vs-p2p | 581 | 43 |
| eviction-policy-lru | 541 | 2 |
| nvme-bar1-bench | 503 | 16 |
| baseline-generalized-fs | 485 | 13 |
| logger | 480 | 2 |
| nvme-ns-manager | 315 | 5 |
| spdk-sys | 280 | 47 |
| gpu-handle-test-server | 76 | 9 |
| example-helloworld | 74 | 2 |
| dynamic-loading-example | 54 | 8 |
| gpu-show | 50 | 0 |
| helloworld-mainline | 26 | 3 |
| example-helloworld-dylib | 8 | 0 |

Top 3 (component-framework, dispatcher, dispatcher-p2p) = 26,207 lines (~38%).

## Applications (Rust)

| App | Code | Tests |
|-----|------|-------|
| certus-server-yaml | 1,443 | 0 |
| certus-server | 1,338 | 491 |
| iops-benchmark-md | 1,221 | 314 |
| iops-benchmark | 1,093 | 304 |
| extent-benchmark | 719 | 0 |
| gpu-bb-vs-p2p | 581 | 0 |
| nvme-bar1-bench | 503 | 0 |
| baseline-generalized-fs | 420 | 0 |
| nvme-ns-manager | 315 | 0 |

## Complexity Indicators

### Largest source files (first-party, by code lines)

| File | Lines |
|------|-------|
| `components/dispatcher/src/lib.rs` | 4,981 |
| `components/dispatcher-p2p/src/lib.rs` | 4,260 |
| `apps/certus-server/src/service.rs` | 1,488 |
| `components/dispatcher-p2p/src/pipeline.rs` | 1,402 |
| `components/block-device-spdk-nvme/src/actor.rs` | 1,313 |
| `components/gpu-services/src/lib.rs` | 1,280 |
| `components/component-framework/.../actor.rs` | 1,273 |

### Highest function count (first-party)

| File | Functions |
|------|-----------|
| `components/dispatcher/src/lib.rs` | 170 |
| `components/dispatcher-p2p/src/lib.rs` | 155 |
| `components/remote-lookup/src/seams.rs` | 98 |
| `components/component-framework/.../actor.rs` | 62 |

### Deepest nesting (first-party)

| File | Max Depth |
|------|-----------|
| `apps/certus-server-yaml/build.rs` | 16 (codegen) |
| `components/dispatcher/src/lib.rs` | 11 |
| `components/dispatcher-p2p/src/lib.rs` | 11 |

### Unsafe usage

- **138 files** contain `unsafe`
- **1,096 total** `unsafe` occurrences
- Concentrated in: SPDK FFI, RDMA, io_uring, CUDA/GPU interop, DMA buffers

## Complexity Hotspots

The two dispatchers (`dispatcher/src/lib.rs`, `dispatcher-p2p/src/lib.rs`) are the
clear complexity concentration: largest files, highest function counts, and deepest
nesting. Both grew notably since the last run (dispatcher 3,992 → 4,981 lines;
dispatcher-p2p 3,747 → 4,260). Candidates for modularization.

## Delta Since 2026-06-18

- New first-party components: `remote-lookup-rdma-initiator/responder`,
  `extended-metadata-store`, `disk-partition-manager`, `block-device-memory`, `zyre`.
- `remote-lookup` grew from ~180 → 2,219 code lines (RDMA remote lookup work).
- Vendored Creusot verifier (`tools/creusot`) added — dominates raw language totals.

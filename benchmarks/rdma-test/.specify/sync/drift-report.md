# Spec Drift Report

Generated: 2026-07-22T21:32:51Z
Project: rdma-test

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 19 (13 FR + 6 SC) |
| Aligned | 16 (84%) |
| Drifted | 3 (16%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 3 features |

Only one spec exists: `specs/001-rdma-network-test/spec.md` ("RDMA Network Test Tool"). Implementation reviewed: `src/main.rs`, `src/rdma.rs`, `src/client.rs`, `src/server.rs`, `src/throughput.rs`, `src/send_bw.rs`, `src/recv_bw.rs`, `src/latency.rs`, `src/stats.rs`, `src/output.rs`, `src/ffi.rs`, `src/wrapper.c`, `build.rs`, `scripts/launch.sh`, `Cargo.toml`, `CLAUDE.md`.

## Detailed Findings

### Spec: 001-rdma-network-test — RDMA Network Test Tool

#### Aligned

- **FR-001** (server/client mode via subcommand) → `src/main.rs:79-93` (`Mode::Server`/`Mode::Client`).
- **FR-002** (RDMA Write throughput, bandwidth/message-rate/total-data) → `src/throughput.rs:9-44` (`run_write_client`), `src/stats.rs:58-76` (`ThroughputStats`).
- **FR-003** (Send/Recv ping-pong latency, min/max/mean/median/P95/P99/jitter) → `src/latency.rs`, `src/stats.rs:5-56` (`LatencyStats::compute`).
- **FR-004** (ibverbs/device availability check with diagnostics) → `src/main.rs:122-190` (`check_ibverbs_available`).
- **FR-006** (auto-detect IB vs RoCE) → `src/main.rs:170-176` (reads `ports/1/link_layer` sysfs attribute).
- **FR-008** (configurable warmup) → `--warmup`/`-w` flag (`src/main.rs:70-72`), used in every test module before timed section.
- **FR-009** (uses libibverbs for all RDMA ops) → `src/ffi.rs` + `src/wrapper.c`, linked via `build.rs:1-26` (`pkg_config::probe_library("libibverbs")`, `librdmacm`).
- **FR-010** (non-zero exit + clear error on failure) → `src/main.rs:262-265`.
- **FR-011** (human default / `--output json`) → `src/output.rs`, `OutputFormat` enum, `src/client.rs:112-133`.
- **FR-013** (server handles exactly one session, then exits) → `src/server.rs:11-58` runs a single `rdma::server_connect` and returns.
- **SC-001/SC-004** (time-bound throughput/launch-script results) — no code path introduces artificial delay beyond `RDMA_TEST_STARTUP_DELAY` (2s default); consistent with the target, but not empirically verified here (requires RDMA hardware).
- **SC-002** (sub-microsecond latency resolution) → `std::time::Instant`-based RTT capture in `src/latency.rs:27-33`, reported to 0.01 µs precision.
- **SC-003** (actionable diagnostics) → `src/main.rs:132-164` prints install/`modprobe rdma_rxe` guidance for missing library/devices.
- **SC-005** (works on IB and RoCE without manual config) → transport auto-detected per FR-006; no transport-specific CLI flag required.
- **SC-006** (JSON parseable by jq/python) → `serde_json::to_string_pretty` in `src/output.rs:78-80` on `#[derive(Serialize)]` structs.

#### Drifted

- **FR-005** — Spec/contract text: `contracts/cli-interface.md:17` documents `--test` as `Enum, default all, values: throughput, latency, all`, and `spec.md:31`/`quickstart.md:51,54` show usage like `--test throughput`.
  - Actual: `TestType` in `src/main.rs:19-27` is `{Write, Read, Send, Recv, Latency, All}`; clap's default `ValueEnum` rendering yields CLI values `write|read|send|recv|latency|all` — **there is no `throughput` value**. Every documented example (`-t throughput`) would fail at argument parsing.
  - Location: `tools/rdma-test/src/main.rs:19-27` vs `tools/rdma-test/specs/001-rdma-network-test/contracts/cli-interface.md:17`
  - Severity: **major** (breaks the documented CLI contract and all example invocations)

- **FR-007** — Spec text: `spec.md:134` states `RDMA_TEST_PORT` default is `50000`.
  - Actual: `scripts/launch.sh:21` (`PORT="${RDMA_TEST_PORT:-7471}"`), `contracts/cli-interface.md:123`, and `src/main.rs:55` (`default_value_t = 7471`) all agree on **7471**. Only the prose in spec.md's own Implementation Details section is out of sync with its own contract and the code.
  - Location: `tools/rdma-test/specs/001-rdma-network-test/spec.md:134` vs `tools/rdma-test/scripts/launch.sh:21`
  - Severity: **minor** (documentation-only inconsistency, no functional impact)

- **FR-012** — Spec text: "System MUST retry failed RDMA connections up to 3 times before aborting, and report partial results if any measurements were collected before failure."
  - Actual: The only 3x-retry loop, `poll_completion_with_retry` (`src/rdma.rs:226-237`), wraps a single completion-queue poll for one work request — it does not retry RDMA-CM connection establishment. `client_connect`/`server_connect` (`src/rdma.rs:331`, `:433`) call `rdma_resolve_addr`/`rdma_resolve_route`/`rdma_connect`/`rdma_accept` exactly once each with no retry if the handshake itself fails. Separately, `TestOutput.partial` is hard-coded to `false` (`src/client.rs:129`) and is never set `true` anywhere in the codebase; on any mid-test error, `main()` (`src/main.rs:262-265`) just prints the `anyhow` error to stderr and exits with code 1 — no JSON or human-readable partial-results output is ever produced.
  - Location: `tools/rdma-test/src/rdma.rs:226-237,331,433`; `tools/rdma-test/src/client.rs:129`; `tools/rdma-test/src/main.rs:262-265`
  - Severity: **major** (the specific documented failure-recovery/partial-reporting behavior does not exist)

#### Not Implemented

None. All 13 functional requirements have corresponding implementation, though FR-005/FR-012 are substantively drifted (see above).

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|-----------------|
| RDMA Read throughput test (`TestType::Read` / `--test read`) | `src/throughput.rs:46-81` (`run_read_client`), wired in `src/client.rs:42-49`, `src/server.rs:26-29` | ~36 | Extend FR-002 in 001-rdma-network-test to cover one-sided RDMA Read, or add a new FR |
| Send-side bandwidth test (`TestType::Send` / `--test send`) — two-sided `ibv_post_send` with pre-posted recv window | `src/send_bw.rs` (full file), wired in `src/client.rs:50-57`, `src/server.rs:30-33` | 74 | New FR in 001-rdma-network-test for two-sided Send/Recv *bandwidth*, distinct from the existing FR-003 Send/Recv *latency* ping-pong |
| Recv-side bandwidth test (`TestType::Recv` / `--test recv`) — two-sided `ibv_post_recv` with pre-posted recv window | `src/recv_bw.rs` (full file), wired in `src/client.rs:58-65`, `src/server.rs:34-37` | 76 | Same as above |

The `--test all` mode (`src/client.rs:74-109`, `src/server.rs:42-53`) runs all five test kinds sequentially in one session; this aggregation itself isn't separately spec'd but follows naturally once Read/Send/Recv bandwidth tests are added to the spec.

## Inter-Spec / Intra-Document Conflicts

1. **Port default mismatch**: `spec.md` (FR-007 Implementation Details, line 134) says `RDMA_TEST_PORT` defaults to `50000`; every other source of truth (`contracts/cli-interface.md:123`, `scripts/launch.sh:21`, `src/main.rs:55`) says `7471`.
2. **`--test` value set mismatch**: `spec.md` (acceptance scenario, line 31), `contracts/cli-interface.md:17`, and `quickstart.md:51,54` all specify/use `throughput` as a `--test` value; the actual `TestType` enum in `src/main.rs:19-27` has no such value (only `write`, `read`, `send`, `recv`, `latency`, `all`).

## Recommendations

1. **Fix the `--test` value drift first (major)** — either rename `TestType::Write` to render as `throughput` (e.g. `#[value(name = "throughput")]` and drop/merge Read/Send/Recv into it), or update `spec.md`, `contracts/cli-interface.md`, and `quickstart.md` to use the real values (`write`, `read`, `send`, `recv`, `latency`, `all`) and formally spec the Read/Send/Recv bandwidth tests as new/extended functional requirements.
2. **Implement true connection-level retry-with-partial-results for FR-012 (major)** — wrap `client_connect`/`server_connect` in a bounded retry loop (reusing the existing `MAX_RETRIES = 3` constant), and thread partial `ThroughputStats`/`LatencyStats` collected so far through the error path in `main.rs` so a `TestOutput { partial: true, error: Some(...) }` is actually emitted (JSON and human) on failure, per the spec's Edge Cases section — or relax FR-012's wording if per-operation completion retry is deemed sufficient and connection-level retry is out of scope.
3. **Reconcile the `RDMA_TEST_PORT` default (minor)** — correct `spec.md:134` from `50000` to `7471` to match the contract, script, and code.
4. **Backfill a spec for the Read/Send/Recv bandwidth tests** — these are fully implemented, tested via the CLI, and documented in code comments, but absent from `spec.md`'s Functional Requirements; use `speckit-sync-backfill` to generate FR text for them so the spec accurately reflects the five test kinds the tool actually supports.

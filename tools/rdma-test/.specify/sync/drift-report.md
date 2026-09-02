---
spec_sync_component: rdma-test
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:46:49Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 30c2e3c28acd9a678ebad7b97b4f08999c4f6d66543ae6d196e006421c3b2e99
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Spec Drift Report

Generated: 2026-09-02T21:46:49Z
Project: rdma-test

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 23 (17 FR + 6 SC) |
| Aligned | 22 (96%) |
| Drifted | 1 (4%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 0 features |
| Stale-doc defects (residual) | 3 |

Only one spec exists: `specs/001-rdma-network-test/spec.md` ("RDMA Network Test
Tool"). This pass re-analyzes the component after the 2026-07-22 backfill that
added FR-014/FR-015/FR-016/FR-017 (RDMA Read + two-sided Send/Recv bandwidth +
`--test` enum contract) — those requirements are now present in the spec and
verified against code. Implementation reviewed: `src/main.rs`, `src/rdma.rs`,
`src/client.rs`, `src/server.rs`, `src/throughput.rs`, `src/send_bw.rs`,
`src/recv_bw.rs`, `src/latency.rs`, `src/stats.rs`, `src/output.rs`,
`src/ffi.rs`, `src/wrapper.c`, `build.rs`, `scripts/launch.sh`, `Cargo.toml`,
plus all spec artifacts (`spec.md`, `plan.md`, `data-model.md`, `quickstart.md`,
`tasks.md`, `contracts/cli-interface.md`, `checklists/requirements.md`).

## Detailed Findings

### Spec: 001-rdma-network-test — RDMA Network Test Tool

#### Aligned

- **FR-001** (server/client mode via subcommand) → `src/main.rs:79-93` (`Mode::Server`/`Mode::Client`), dispatched at `src/main.rs:240-260`.
- **FR-002** (RDMA Write throughput; bandwidth/msg-rate/total-data) → `src/throughput.rs:9-44` (`run_write_client`), `src/stats.rs:58-76` (`ThroughputStats`: `bandwidth_gbps`, `message_rate_mpps`, `total_bytes`).
- **FR-014** (one-sided RDMA Read throughput, `--test read`, same metrics) → `src/throughput.rs:46-81` (`run_read_client`), wired `src/client.rs:42-49`, `src/server.rs:26-29`.
- **FR-015** (two-sided Send bandwidth, `--test send`; client posts sends against server pre-posted recv window) → `src/send_bw.rs:11-37` (`run_client` posts sends), `:39-74` (`run_server` pre-posts `RECV_WINDOW` recvs then signals ready), wired `src/client.rs:50-57`, `src/server.rs:30-33`.
- **FR-016** (two-sided Recv bandwidth, `--test recv`; client pre-posts recv window, server drives sends) → `src/recv_bw.rs:11-54` (`run_client` pre-posts recvs), `:56-76` (`run_server` drives `total` sends), wired `src/client.rs:58-65`, `src/server.rs:34-37`.
- **FR-017** (`--test` accepts exactly write/read/send/recv/latency/all; `all` default; `all` runs all five sequentially in one session) → `TestType` enum has exactly those six variants (`src/main.rs:19-27`), default `"all"` (`src/main.rs:67`), `TestType::All` runs write→read→send→recv→latency on one connection (`src/client.rs:74-109`, `src/server.rs:42-53`).
- **FR-003** (Send/Recv ping-pong latency; min/max/mean/median/P95/P99/jitter) → `src/latency.rs:9-37`, `src/stats.rs:17-56` (`LatencyStats::compute`).
- **FR-004** (ibverbs/device availability check with diagnostics at startup) → `src/main.rs:122-190` (`check_ibverbs_available`), called `src/main.rs:216`.
- **FR-005** (configure size/iterations/warmup/test/port/output + optional device via CLI) → all global flags present `src/main.rs:50-77`.
- **FR-006** (auto-detect IB vs RoCE) → `src/main.rs:170-176` (reads `ports/1/link_layer` sysfs attribute).
- **FR-007** (SSH launch script starts remote server + client, collects results) → `scripts/launch.sh` (server bg launch `:72-75`, readiness check `:80-85`, client run `:89-90`, output/cleanup `:54-59,94-105`). Port default `7471` now consistent across `spec.md:143`, `contracts/cli-interface.md:147`, `launch.sh:21`, `src/main.rs:55`.
- **FR-008** (configurable warmup) → `--warmup`/`-w` (`src/main.rs:70-72`); warmup loop precedes timed section in every module (`throughput.rs:25`, `send_bw.rs:25`, `recv_bw.rs:33`, `latency.rs:19`).
- **FR-009** (uses libibverbs for all RDMA ops) → `src/ffi.rs` + `src/wrapper.c`, linked via `build.rs:1-26` (`pkg_config::probe_library("libibverbs")` + `librdmacm`).
- **FR-010** (non-zero exit + clear error on failure) → `src/main.rs:262-265` (exit 1 on error), `:207-214` (exit 2 on validation).
- **FR-011** (human default / `--output json`) → `src/output.rs` (`OutputFormat`, `TestOutput`), `src/client.rs:112-133`.
- **FR-013** (server handles exactly one session, then exits) → `src/server.rs:19` runs a single `rdma::server_connect` then returns; no accept loop.
- **SC-001** (throughput result in <30s for 10k iters) — no code path introduces artificial delay; consistent with target but not empirically verified (needs RDMA hardware).
- **SC-002** (sub-µs latency resolution) → `std::time::Instant` RTT capture `src/latency.rs:28-33`, reported to 0.01 µs (`src/stats.rs:46-52`).
- **SC-003** (actionable diagnostics) → `src/main.rs:132-164` prints install / `modprobe rdma_rxe` guidance for missing library/devices.
- **SC-004** (launch script <45s time-to-first-result) → only `RDMA_TEST_STARTUP_DELAY` (2s default) delay; consistent with target, not empirically verified.
- **SC-005** (works on IB and RoCE without manual transport config) → transport auto-detected per FR-006; no transport-specific CLI flag.
- **SC-006** (JSON parseable by jq/python) → `serde_json::to_string_pretty` on `#[derive(Serialize)]` structs (`src/output.rs:78-80`, `:11-41`). Note: the quickstart example key was stale (see stale-doc D2 below); the emitted structure itself is correct.

#### Drifted

- **FR-012** — Spec text (FR-012 + Edge Cases + 2026-06-05 clarification): "System MUST retry failed RDMA connections up to 3 times before aborting, and report partial results if any measurements were collected before failure."
  - Actual: The only 3× retry loop, `poll_completion_with_retry` (`src/rdma.rs:226-237`), wraps a single completion-queue poll for one work request — it does **not** retry RDMA-CM connection establishment. `client_connect`/`server_connect` (`src/rdma.rs:433`, `:331`) call `rdma_resolve_addr`/`rdma_resolve_route`/`rdma_connect`/`rdma_accept` exactly once each with no retry/backoff if the handshake fails. Separately, `TestOutput.partial` is hard-coded to `false` (`src/client.rs:129`) and never set `true`; on any mid-test error `main()` (`src/main.rs:262-265`) prints the `anyhow` error to stderr and exits 1 — no partial JSON or human-readable results are ever emitted.
  - Location: `tools/rdma-test/src/rdma.rs:226-237,331,433`; `tools/rdma-test/src/client.rs:129`; `tools/rdma-test/src/main.rs:262-265`
  - Severity: **major** (documented failure-recovery / partial-reporting behavior does not exist)
  - Disposition: **ALIGN** — pre-existing implementation task in `.specify/sync/align-tasks.md`. The spec deliberately documents the *intended* behavior (per hard rules); the gap is also flagged in `contracts/cli-interface.md:134` ("Known gap") and in `tasks.md` (T032 note). Unchanged this pass.

#### Not Implemented

None. All 17 functional requirements have corresponding implementation. FR-012 is implemented only partially (per-operation completion retry exists; connection-level retry + partial reporting do not).

### Unspecced Code

None. The three features flagged as unspecced in the 2026-07-22 report (RDMA Read, two-sided Send bandwidth, two-sided Recv bandwidth) are now covered by FR-014/FR-015/FR-016 and the `--test all` aggregation by FR-017.

### Residual Stale-Documentation Defects

These are leftovers from the 2026-07-22 backfill that corrected most `throughput`
references but missed three. The underlying requirements (FR-011/FR-017/SC-006)
are correctly implemented; only the documentation was stale.

| ID | Location | Issue | Disposition |
|----|----------|-------|-------------|
| D1 | `specs/001-rdma-network-test/tasks.md:61` | US1 checkpoint used `-t throughput` (invalid enum value) | **BACKFILL** — corrected to `-t write` (this pass) |
| D2 | `specs/001-rdma-network-test/quickstart.md:74` | JSON example `jq .results.throughput.bandwidth_gbps` referenced a nonexistent key; actual key is `write` | **BACKFILL** — corrected to `.results.write.bandwidth_gbps` (this pass) |
| D3 | `scripts/launch.sh:11` | usage-header comment example `--test throughput` (invalid enum value) | **ALIGN** — code file; recorded in `align-tasks.md` (not edited by spec-only backfill) |

## Inter-Spec / Intra-Document Conflicts

None outstanding. The 2026-07-22 port-default conflict (`50000` vs `7471`) and the primary `--test` value-set conflict were resolved by the prior backfill; the three residual `throughput` references above are the only remaining trace and are addressed as D1/D2/D3.

## Recommendations

1. **Implement the FR-012 align task** (connection-level retry + partial-results-on-failure), then re-run spec-sync so FR-012 moves to Aligned. See `.specify/sync/align-tasks.md`.
2. **Fix the `scripts/launch.sh:11` comment** (D3) — trivial one-line edit to a valid `--test` value; recorded in `align-tasks.md`.
3. No further spec edits required; FR-014–FR-017 verified accurate against the implementation.

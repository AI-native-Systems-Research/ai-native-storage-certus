# Drift Report: certus-server

**Generated**: 2026-07-22
**Specs analyzed**: 3 (001-grpc-dispatcher-server, 002-operational-config, 003-otel-observability)
**Requirements checked**: 61 (48 FR + 13 SC)

## Summary

| Status | Count |
|--------|-------|
| Aligned | 57 |
| Drifted | 4 |
| Not Implemented | 0 |
| Unspecced Features | 5 |
| Conflicts (spec vs. docs) | 1 |

Scope: `apps/certus-server/src/{main.rs,service.rs,telemetry.rs}`, `apps/certus-server/proto/dispatcher.proto`, `apps/certus-server/Cargo.toml`, `apps/certus-server/README.md`. `python-client/` is in scope only where a spec (spec 001, FR-010) explicitly references it.

---

## Spec 001: gRPC Dispatcher Server

**Overall**: 20/24 FR aligned, 6/6 SC aligned, 4 FR drifted.

### Aligned Requirements

- **FR-001**–**FR-007, FR-005b**: All batch RPCs (`Populate`, `Lookup`, `Check`, `Remove`, `Touch`) map 1:1 to `IDispatcher` methods with per-entry `EntryResult`; no lifecycle RPCs are exposed. `src/service.rs:202-532`.
- **FR-008b, FR-008c**: Dispatch map is created fresh (`DispatchMapState::default()`, no extent manager) and the memory-tier pool is registered via `cudaHostRegister` with a warn-and-continue fallback. `src/main.rs:198-215, 241-261`.
- **FR-009**: SIGTERM/SIGINT triggers `serve_with_shutdown` then `dispatcher.shutdown()`. `src/main.rs:406-426`.
- **FR-013**: Dispatcher is `Arc<dyn IDispatcher + Send + Sync>`; handlers run via `spawn_blocking`, serializing only on the IPC-cache and pending-stores mutexes. `src/service.rs`.
- **FR-014**: TLS enabled only when both `--tls-cert` and `--tls-key` are present. `src/main.rs:396-402`.
- **FR-015**: `check_duplicate_keys()` rejects any batch with a repeated key, applied uniformly across populate/lookup/check/remove/touch/reserve/copy_to_store/commit_store/abort_store/pin/unpin. `src/service.rs:151-162`.
- **FR-016**: `--device-pci` is repeatable and flows into `DispatcherConfig.data_pci_addrs`. `src/main.rs:31-33, 304-311`.
- **FR-017**: `ClearMemoryTier` returns `entries_cleared: u64`. `src/service.rs:772-792`.
- **FR-018, FR-019**: Global `ipc_cache: Arc<Mutex<HashMap<[u8;64], IpcCacheEntry>>>` with refcounting; `cudaSetDevice` is called before opening an uncached handle when `gpu_device_id >= 0`, and failures propagate as `IoError`. `src/service.rs:88-149`.
- **FR-021**: `Pin`/`Unpin` implemented with duplicate-key checks and a `promote` flag that fires a background `promote_to_memory_tier`. `src/service.rs:814-870`.
- **FR-022**: `TakeEvents` performs a non-blocking drain of a bounded (16384) crossbeam channel and reports `dropped_count` via `AtomicU64`. `src/main.rs:357-358`; `src/service.rs:872-908`.
- **SC-001 – SC-006**: Structurally consistent with the batch API design (single round trip per op type, per-entry error correlation, multi-device support). Timing-based criteria (SC-004 "ready within 10s") are not falsifiable via static review but nothing in the code contradicts them.

### Drifted Requirements

| Requirement | Spec says | Actual | Location | Severity |
|---|---|---|---|---|
| FR-008 | Init stack is: SPDK, GPU services, dispatch map, memory-tier, dispatcher. | Also constructs and binds an `EvictionPolicyLru` component (shared by dispatch-map and memory-tier) and a `RemoteLookup` component (bound to the dispatcher); neither appears in the spec's stack description or "Component Stack" section. | `src/main.rs:187-270` | Low |
| FR-010 | Python client CLI is `[--server ADDR] [--skip-tests] [--benchmark]`; 9 named test functions incl. `test_populate_already_exists`; exit codes 0/1/2/3. | Actual flags: `--server`, `--skip-large-batch`, `--bench`, `--bench-only`, `--bench-object-size`, `--bench-num-objects`, `--bench-iterations`. 10 differently-named test functions (`test_batch_populate`, `test_batch_check`, `test_batch_touch`, `test_batch_lookup`, `test_batch_remove`, `test_check_after_remove`, `test_duplicate_key_rejection`, `test_nonexistent_key_handling`, `test_touch_nonexistent`, `test_large_batch`); no AlreadyExists test case; only exit codes 0/1 are used. | `python-client/test_client.py:465-543, 87-303` | Medium |
| FR-011 | `IpcHandle` has exactly 3 fields: `cuda_ipc_handle`, `size`, `gpu_device_id`. | Proto defines a 4th field, `offset` (uint64), actively used to address a sub-block within one shared CUDA allocation (`dev_ptr + offset`). | `proto/dispatcher.proto:59-70`; `src/service.rs:275,375,651` | Low |
| FR-020 | Pending stores are tracked in a `PendingStores` map keyed by a **server-assigned reservation ID**. | `PendingStores` is keyed by the **client-supplied cache key** from `ReserveEntry.key`; `CommitStore`/`AbortStore` take cache `keys`, not reservation IDs. No reservation-ID allocation exists anywhere in the service. | `src/service.rs:48-52,546-559,691-720`; `proto/dispatcher.proto:161-193` | Medium |

---

## Spec 002: Operational Configuration & Lifecycle

**Overall**: 13/13 FR aligned, 4/4 SC aligned. No drift found.

- **FR-001 – FR-004**: `--drive-count` and `--device-pci` are mutually exclusive (`conflicts_with`), NVMe auto-discovery filters PCI class `0x010802` and sorts NUMA-0 first, insufficient-device case returns a descriptive error, and omitting both flags is rejected. `src/main.rs:31-37,104-175,318-330`.
- **FR-005 – FR-008**: `--format`, `--memory-tier-size` (K/M/G parsing, 2 GiB default), `--poller-base-cpu`, and `--max-eviction-attempts` (default 2048) are all present and threaded into `DispatcherConfig`. `src/main.rs:43-69,86-102,304-312,339-344`.
- **FR-009**: PCI address format validated via `parse_pci_address`/`validate_pci_address`. `src/main.rs:104-127`.
- **FR-010, FR-011**: `FlushToSsd` and `Touch{promote}` implemented as specified, including fire-and-forget promotion via `spawn_blocking`. `src/service.rs:490-532,794-812`.
- **FR-012, FR-013**: CUDA host-registration fallback and NUMA-aware memory-tier binding (matches first selected drive's NUMA node) both present. `src/main.rs:230-261`.

---

## Spec 003: OpenTelemetry Observability

**Overall**: 11/11 FR aligned, 3/3 SC aligned. No drift found.

- **FR-001 – FR-004**: `otel` is a genuine compile-time feature (`#[cfg(feature = "otel")]` throughout `main.rs`/`service.rs`); endpoint/service-name flags and the 10-second `PeriodicReader` interval match. `Cargo.toml:8-13`; `src/telemetry.rs:49-72`.
- **FR-005 – FR-009**: All 16 documented instruments (6 dispatcher-level + 10 pipeline-stage) exist with matching names/attributes: `ops_total`, `ops_errors`, `op_duration_us`, `batch_size`, `entries_cleared`, `jobs_flushed`, and the cold/hot/populate pipeline histograms. `src/telemetry.rs:18-42,76-151`.
- **FR-010**: `disp_comp.set_pipeline_metrics(...)` wires `PipelineStageMetrics` into the dispatcher. `src/main.rs:370`.
- **FR-011**: `entries_cleared`/`jobs_flushed` are seeded with `.add(0, &[])` at init. `src/telemetry.rs:106-108`.
- **SC-001**: Metric count is exactly 16, matching the spec's success criterion.

---

## Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---|---|---|---|
| `GetIoStats` RPC + `IoStatsResponse` (per-direction SSD read/write op/byte/latency counters), gated by the optional `rw-telemetry` Cargo feature | `proto/dispatcher.proto`; `src/service.rs:910-927`; `Cargo.toml:16` | ~25 | Add an FR to spec 003, or a new spec, covering `GetIoStats` and `rw-telemetry` |
| `--memory-tier-eviction-threshold` CLI flag (background DRAM→SSD demotion threshold, default disabled at 0.0) | `src/main.rs:71-74,136,309` | ~4 | Add FR + CLI table row to spec 002 |
| `IpcHandle.offset` field (sub-block addressing within one shared allocation) | `proto/dispatcher.proto:59-70` | ~12 | Fold into spec 001 FR-011's field list |
| `EvictionPolicyLru` and `RemoteLookup` components wired into the startup stack but absent from the documented "Component Stack" | `src/main.rs:187-196, 263-270` | ~20 | Extend spec 001's Component Stack section |
| `ERROR_CODE_DUPLICATE_KEY` enum value defined but never returned (duplicate-key rejection actually uses `Status::invalid_argument`) | `proto/dispatcher.proto:77`; `src/service.rs:151-162` | 1 | Wire it in, or remove it and document the actual `invalid_argument` behavior in FR-015 |

## Conflicts (Spec vs. Documentation)

- **README.md vs. spec 002 / actual code**: README's "Component Stack" section and architecture diagram still describe a metadata NVMe device plus an `IExtentManager` persistence layer backing the dispatch map — directly contradicting spec 002's clarification that there is *no* metadata device and *no* extent-manager-backed persistence (dispatch map is ephemeral). README's gRPC API table lists only 4 of the 15 defined RPCs (missing `Touch`, `Reserve`, `CopyToStore`, `CommitStore`, `AbortStore`, `ClearMemoryTier`, `FlushToSsd`, `Pin`, `Unpin`, `TakeEvents`, `GetIoStats`), and its CLI Options table omits `--drive-count`, `--format`, `--memory-tier-size`, `--poller-base-cpu`, `--max-eviction-attempts`, `--memory-tier-eviction-threshold`, `--otel-endpoint`, `--otel-service-name`. (`README.md:9-72,100-108`)

## Recommendations

1. **Fix `python-client/test_client.py` documentation drift (FR-010)** — highest-value fix since it will actively mislead anyone trying to run the client per the spec's Implementation Details. Either update spec 001's FR-010 section to match the real CLI/test names, or rename the test functions/flags to match the spec.
2. **Clarify FR-020's "reservation ID" language** — either implement true server-generated reservation IDs, or update the spec text to say pending stores are keyed by the client-supplied cache key (current, working behavior).
3. **Add `offset` to FR-011** and document its sub-block-addressing purpose; it's a real, load-bearing protocol field.
4. **Rewrite `README.md`'s Component Stack, gRPC API table, and CLI Options table** to match the current no-extent-manager, 15-RPC, 12-flag implementation — it currently describes an architecture that predates spec 002's clarifications.
5. **Backfill the 5 unspecced items** (`GetIoStats`/`rw-telemetry`, `--memory-tier-eviction-threshold`, eviction-policy/remote-lookup in the component stack, and the unused `ERROR_CODE_DUPLICATE_KEY`) into specs 001–003, or explicitly mark them as intentionally undocumented internals.
6. Update `apps/certus-server/CLAUDE.md`'s "Recent Changes" section, which still only references spec 001 (last updated 2026-05-05) despite specs 002 and 003 existing.

# Drift Resolution Proposals — interfaces

Generated: 2026-10-21  
Branch: `001-rdma-cache-transfer` (vs `origin/unstable`)  
Based on: drift-report.md § *Sync 2026-10-21 (branch 001-rdma-cache-transfer)*  
Status: **proposed, none approved, nothing applied**

| Type | Count |
|---|---|
| BACKFILL (code → spec) | 7 |
| ALIGN (doc comment in src) | 2 |
| HUMAN_DECISION | 3 |

| # | Requirement | Direction | Severity | Location |
|---|---|---|---|---|
| 1 | Overview: spdk feature gate list | BACKFILL | moderate | `components/interfaces/specs/001-interfaces/spec.md:29` |
| 2 | FR-007 heading | BACKFILL | moderate | `components/interfaces/specs/001-interfaces/spec.md:148` |
| 3 | FR-009 IMemoryTier heading + method inventory | BACKFILL | major | `components/interfaces/specs/001-interfaces/spec.md:191,208` |
| 4 | FR-008 IDispatcher: read_to_bounce / release_bounce | BACKFILL | major | `components/interfaces/specs/001-interfaces/spec.md:189` |
| 5 | FR-019 Memory-tier supporting types | BACKFILL | major | `components/interfaces/specs/001-interfaces/spec.md:335-337` |
| 6 | NEW FR-035 / FR-036: ITransferEndpoint, ITransferEndpointAdmin and supporting types | BACKFILL | major | `components/interfaces/specs/001-interfaces/spec.md:474 (insert after FR-034)` |
| 7 | NFR-003 Thread safety | BACKFILL | minor | `components/interfaces/specs/001-interfaces/spec.md:487` |
| 8 | FR-008 IDispatcher: pre-existing inventory gaps (batch_populate, tier_event_stats, copy_gpu_to_memory_async signature) | HUMAN_DECISION | moderate | `components/interfaces/specs/001-interfaces/spec.md:171-189 (and US2 list at :49)` |
| 9 | DEFAULT_BOUNCE_RESERVE sizing rationale vs FR-027 8 MiB block | HUMAN_DECISION | moderate | `components/interfaces/specs/001-interfaces/spec.md:406 (FR-027)` |
| 10 | IDispatcher::read_to_bounce doc: error list | ALIGN | minor | `(code doc comment)` |
| 11 | ITransferEndpointAdmin configure/initialize error docs | ALIGN | minor | `(code doc comment)` |
| 12 | plan.md crate structure + interface count table | HUMAN_DECISION | moderate | `components/interfaces/specs/001-interfaces/plan.md:8,17-32,42-55` |

### 1. Overview: spdk feature gate list — BACKFILL

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:29`
- **Code location:** `components/interfaces/src/lib.rs:28-34,58; src/idispatch_map.rs:81`
- **Severity / confidence:** moderate / HIGH
- **Rationale:** Code is authoritative: the ungating is deliberate (lib.rs comment, certus-transfer-endpoint tasks T006/plan.md:235). IMemoryTier was already ungated on origin/unstable; corrected here because the new ungating cites it as precedent.

**Current:**

```text
- **`spdk`** (optional): Gates SPDK-dependent interfaces and types (`IBlockDevice`, `IBlockDeviceAdmin`, `ISPDKEnv`, `IDispatcher`, `IDispatchMap`, `IMemoryTier`, `IExtentManager`, `IPartitionTable`) and supporting types (`DmaBuffer`, `PciAddress`, etc.).
```

**Proposed:**

```text
- **`spdk`** (optional): Gates SPDK-dependent interfaces and types (`IBlockDevice`, `IBlockDeviceAdmin`, `ISPDKEnv`, `IDispatcher`, `IExtentManager`, `IPartitionTable`) and supporting types (`DmaBuffer`, `PciAddress`, etc.). Interfaces that merely carry opaque pointers or plain data are **not** gated even where their implementations need SPDK: `IMemoryTier`, `IDispatchMap`/`LookupResult` *(ungated 2026-10-21, branch `001-rdma-cache-transfer`, so `certus-transfer-endpoint` can consume them as a default workspace member without `spdk-sys`)*, and `ITransferEndpoint`/`ITransferEndpointAdmin` (FR-035).
```

- **Approved:** false

### 2. FR-007 heading — BACKFILL

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:148`
- **Code location:** `components/interfaces/src/idispatch_map.rs:81,203,211`
- **Severity / confidence:** moderate / HIGH
- **Rationale:** #[cfg(feature = "spdk")] removed from the trait, LookupResult and its Send/Sync impls.

**Current:**

```text
#### FR-007: IDispatchMap Interface (feature: spdk)
```

**Proposed:**

```text
#### FR-007: IDispatchMap Interface (ungated; `set_checksum`/`get_checksum` feature: `integrity-check`)
```

- **Approved:** false

### 3. FR-009 IMemoryTier heading + method inventory — BACKFILL

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:191,208`
- **Code location:** `components/interfaces/src/imemory_tier.rs:220-238`
- **Severity / confidence:** major / HIGH
- **Rationale:** Four methods added for block-tier peer serving (certus-transfer-endpoint FR-012, T083); implemented by memory-tier (lib.rs:291 caps at pool_size/8, :687 NotInitialized). Heading 'feature: spdk' was already false on unstable (entangled).

**Current:**

```text
#### FR-009: IMemoryTier Interface (feature: spdk)
...
- **Method**: `telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot` - Return a snapshot of cumulative eviction and lock-contention counters (zeroed unless built with the `telemetry` feature).
```

**Proposed:**

```text
#### FR-009: IMemoryTier Interface (ungated)
...
- **Method**: `telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot` - Return a snapshot of cumulative eviction and lock-contention counters (zeroed unless built with the `telemetry` feature).
- **Method**: `set_bounce_reserve(&self, bytes: usize) -> Result<(), MemoryTierError>` *(New 2026-10-21)* - Override the bounce-reserve size (default `DEFAULT_BOUNCE_RESERVE`, FR-019). Only before `initialize`, which takes `min(bytes, pool_size / 8)` from the top of the pool; afterwards an error. `0` means no reserve.
- **Method**: `bounce_alloc(&self, len: u32) -> Result<BounceLease, BounceError>` *(New 2026-10-21)* - Lease `len` bytes (rounded up to 4 KiB) of the reserve, which lies inside the pool but outside the cache, so a lease is DMA-able and covered by any whole-pool registration and taking one never evicts. `NoSpace` if it does not fit now; `NeverFits` if it never can (larger than the reserve, no reserve, or a pool that is not DMA-capable); `NotInitialized` before `initialize`.
- **Method**: `bounce_free(&self, lease: BounceLease)` *(New 2026-10-21)* - Return a lease; only once no device or NIC can still access its bytes.
- **Method**: `bounce_usage(&self) -> (usize, usize)` *(New 2026-10-21)* - Reserve size and bytes leased now, both after 4 KiB rounding.
```

- **Approved:** false

### 4. FR-008 IDispatcher: read_to_bounce / release_bounce — BACKFILL

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:189`
- **Code location:** `components/interfaces/src/idispatcher.rs:879-907`
- **Severity / confidence:** major / HIGH
- **Rationale:** Added on this branch (commit 23b5eeb6); the contract is owned by certus-transfer-endpoint FR-012 and contracts/itransfer_endpoint.md:363-372. Implemented by dispatcher (lib.rs:3988); dispatcher-p2p and remote-lookup seams stub it.

**Current:**

```text
- **Method**: `read_write_stats(&self) -> ReadWriteStats` - Return cumulative per-direction SSD read/write byte, op, and latency counters aggregated across all data drives (zeroed unless built with the `telemetry` feature; monotonic).
```

**Proposed:**

```text
- **Method**: `read_write_stats(&self) -> ReadWriteStats` - Return cumulative per-direction SSD read/write byte, op, and latency counters aggregated across all data drives (zeroed unless built with the `telemetry` feature; monotonic).
- **Method**: `read_to_bounce(&self, key: CacheKey, offset: u64, len: u32) -> Result<BounceLease, BounceError>` *(New 2026-10-21)* - Read the block-tier entry `key`, `len` bytes at extent `offset`, into a fresh bounce lease (FR-009) for serving a peer. The caller MUST hold a pin on `key` (its `IDispatchMap::lookup` `BlockDevice { offset }` answer) for the whole call; this method takes and releases none. Never promotes and never touches the cache, so serving a peer cannot evict. Blocks until the read completes — deliberately no device timeout, since freeing a lease the drive may still write is unsafe. Errors: `NoSpace` (retry), `NeverFits` (never satisfiable), `Io` (read failed; lease freed, or deliberately leaked if the drive may still write it), `NotInitialized`.
- **Method**: `release_bounce(&self, lease: BounceLease)` *(New 2026-10-21)* - Free a lease from `read_to_bounce` once nothing can still read it (its write has completed).
```

- **Approved:** false

### 5. FR-019 Memory-tier supporting types — BACKFILL

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:335-337`
- **Code location:** `components/interfaces/src/imemory_tier.rs:52-135; src/lib.rs:61`
- **Severity / confidence:** major / HIGH
- **Rationale:** Three new public items re-exported from lib.rs:61 with no spec coverage.

**Current:**

```text
#### FR-019: Supporting Types - Memory Tier
- `MemoryTierError`: 7-variant error enum.
- `MemoryTierTelemetrySnapshot`: `Copy + Default` 3-field cumulative counter snapshot (`evictions`, `write_lock_contentions`, `read_lock_contentions`), returned by `IMemoryTier::telemetry_snapshot` (see FR-009); zeroed unless the `telemetry` feature is enabled.
```

**Proposed:**

```text
#### FR-019: Supporting Types - Memory Tier
- `MemoryTierError`: 7-variant error enum.
- `MemoryTierTelemetrySnapshot`: `Copy + Default` 3-field cumulative counter snapshot (`evictions`, `write_lock_contentions`, `read_lock_contentions`), returned by `IMemoryTier::telemetry_snapshot` (see FR-009); zeroed unless the `telemetry` feature is enabled.
- `BounceLease` *(New 2026-10-21)*: `Copy` 2-field range of the bounce reserve (`ptr: *mut u8`, `len: u32` — the length requested, which must be passed back unchanged when freed); `Send` but not `Sync` (NFR-003).
- `BounceError` *(New 2026-10-21)*: 4-variant error enum (`NoSpace`, `NeverFits(String)`, `Io(String)`, `NotInitialized(String)`) with `is_transient()` true only for `NoSpace`. Shared by `IMemoryTier::bounce_alloc` and `IDispatcher::read_to_bounce`.
- `DEFAULT_BOUNCE_RESERVE` *(New 2026-10-21)*: `usize` constant, 16 MiB — the reserve taken at `initialize` unless overridden by `set_bounce_reserve`, capped at an eighth of the pool.
```

- **Approved:** false

### 6. NEW FR-035 / FR-036: ITransferEndpoint, ITransferEndpointAdmin and supporting types — BACKFILL

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:474 (insert after FR-034)`
- **Code location:** `components/interfaces/src/itransfer_endpoint.rs:1-699; src/lib.rs:23,75-88`
- **Severity / confidence:** major / MEDIUM
- **Rationale:** Whole new module with 11 re-exported items and no coverage in this component's spec. Inventory-level FR mirrors how FR-030..FR-034 cover the RDMA interfaces; behaviour stays with the owning component spec to avoid a taxonomy written twice. MEDIUM because the placement/depth (inventory here vs. pointer only) is a style choice.

**Current:**

```text
(none — no requirement mentions ITransferEndpoint)
```

**Proposed:**

```text
#### FR-035: ITransferEndpoint / ITransferEndpointAdmin Interfaces *(New 2026-10-21)*
- Ungated (receptacles of the implementor are `ILogger`/`IDispatchMap`/`IMemoryTier`, all available without `spdk`). The behavioural contract — wire protocol, credits, deadlines, teardown-before-reclaim — is owned by `components/certus-transfer-endpoint/specs/001-rdma-cache-transfer` (spec + `contracts/itransfer_endpoint.md`) and is referenced, not restated, here.
- `ITransferEndpoint` (6 methods): `register_landing_region(base: *mut u8, len: usize) -> Result<LandingRegionHandle, TransferEndpointError>`; `fetch(peer: &str, items: &[(CacheKey, RemoteRegion)], on_complete: FetchCompletion) -> Result<TransferContextId, TransferEndpointError>` (destination side; accepts any length, chunked by the peer's `max_entries`; `on_complete` fires exactly once on `Ok`, never on `Err`); `push_async(endpoint, items, on_complete: FetchCompletion) -> Result<(), TransferEndpointError>` and `push(endpoint, items) -> Result<Vec<EntryOutcome>, TransferEndpointError>` (legacy source-initiated subset, signature-compatible with `IRemoteLookupRdmaInitiator`); `disconnect(peer: &str) -> Result<(), TransferEndpointError>`; `stats() -> TransferStats`. There is deliberately no `serve` method: the source side is reactive. Every `peer`/`endpoint` is a normalized `"host:port"`; a missing port is `InvalidEndpoint`.
- `ITransferEndpointAdmin` (3 methods): `configure(config: EndpointConfig)`, `initialize()`, `shutdown()` (the only way to stop a running endpoint; idempotent). Until `initialize` succeeds every `ITransferEndpoint` method returns `NotInitialized`.

#### FR-036: Supporting Types - Transfer Endpoint *(New 2026-10-21)*
- `EntryOutcome`: `Copy` 4-variant per-entry outcome (`HitMemoryTier`, `HitBlockTier`, `Miss`, `Failed`); `is_hit()` true for the first two. `Failed` is kept apart from `Miss` so a broken path is never read as a cold cache.
- `TransferContextId`: `Copy + Ord + Hash` newtype over `u64` naming one fetch; a correlation id, not a cancellation handle.
- `LandingRegionHandle`: `Copy` handle (`id`, `addr`, `len`, `rkey`, private fields with accessors) with bounds-checked `region_at(offset, len) -> Option<RemoteRegion>`; valid across reconnects.
- `TransferEndpointError`: 6-variant error enum (`NotInitialized`, `InvalidEndpoint`, `Deadline`, `Protocol`, `IncompatiblePeer`, `Transport`), each carrying a `String`.
- `FetchResult`: alias `Result<Vec<EntryOutcome>, TransferEndpointError>`; `FetchCompletion`: `Box<dyn FnOnce(FetchResult) + Send>`, run on the endpoint thread and must not block.
- `TransferStats`: `Copy + Default` 22-field counter snapshot (5 entry-outcome counters, 3 request/chunk counters, 4 in-flight/peak gauges for send queue and outbound/inbound requests, 2 bounce-buffer gauges, 4 block-read counters, and `deadline_expiries`, `protocol_faults`, `version_downgrades`, `duplicate_connections_resolved`).
- `EndpointConfig`: 10-field start-up configuration (`local`, `max_send_wr`, `max_recv_wr`, `send_credits`, `message_credits`, `inbound_quota`, `actor_cpu`, `fetch_deadline`, `deadline_action`, `block_readers`); `EndpointConfig::new(local)` defaults: 128 / 64 / 8 / 4 / 4 / `None` / `Some(5 s)` / `FailLoud` / 4 (credit defaults sized by measurement, certus-transfer-endpoint research D10).
- `DeadlineAction`: 2-variant enum, `FailLoud` (default) and `ReportPartial` (deferred; refused at `configure`, never silently run as `FailLoud`).
- `RemoteRegion` (FR-033) is reused verbatim, not redefined.
```

- **Approved:** false

### 7. NFR-003 Thread safety — BACKFILL

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:487`
- **Code location:** `components/interfaces/src/imemory_tier.rs:84-86`
- **Severity / confidence:** minor / HIGH
- **Rationale:** New unsafe impl not covered by the enumerated list.

**Current:**

```text
- `DmaBuffer`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`, `LookupResult` are `Send + Sync`.
```

**Proposed:**

```text
- `DmaBuffer`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`, `LookupResult` are `Send + Sync`.
- `BounceLease` is `Send` (hand-written, SAFETY-justified: whoever holds a lease owns its bytes until freed, and the pool outlives every lease) but deliberately **not** `Sync`.
```

- **Approved:** false

### 8. FR-008 IDispatcher: pre-existing inventory gaps (batch_populate, tier_event_stats, copy_gpu_to_memory_async signature) — HUMAN_DECISION

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:171-189 (and US2 list at :49)`
- **Code location:** `components/interfaces/src/idispatcher.rs:688,732,877`
- **Severity / confidence:** moderate / HIGH
- **Rationale:** Pre-existing on origin/unstable (stamped clean there), so out of strict scope; entangled only because P4 edits the same inventory and a reviewer would expect the list to be complete after it. Question: fix in this PR (forces re-stamp of all components anyway since interfaces changed) or defer?

**Current:**

```text
(batch_populate and tier_event_stats absent); `copy_gpu_to_memory_async(&self, key: CacheKey, ipc_handle: IpcHandle, stream: GpuStream)`
```

**Proposed:**

```text
Option A (fix while here): add `batch_populate(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` and `tier_event_stats(&self) -> TierEventStats`, and correct `copy_gpu_to_memory_async(&self, key: CacheKey, regions: &[IpcHandle], stream: GpuStream)`. Option B: leave for a separate pre-existing-drift pass.
```

- **Approved:** false

### 9. DEFAULT_BOUNCE_RESERVE sizing rationale vs FR-027 8 MiB block — HUMAN_DECISION

- **Spec location:** `components/interfaces/specs/001-interfaces/spec.md:406 (FR-027)`
- **Code location:** `components/interfaces/src/imemory_tier.rs:52-55; src/itransfer_endpoint.rs:476-479,518`
- **Severity / confidence:** moderate / MEDIUM
- **Rationale:** Not a correctness bug — NoSpace is transient and retried — but the stated premise contradicts a measured fact recorded in this same spec, and with the defaults half the block readers cannot get a lease at 8 MiB. Design choice belongs to the user.

**Current:**

```text
Doc: "Default size of the bounce reserve: four concurrent device reads of up to 4 MiB, the largest KV block expected."
```

**Proposed:**

```text
Option A: keep 16 MiB, correct the doc to say it fits 4 reads of 4 MiB or 2 of the 8 MiB per-worker block FR-027 records, the rest deferring (counted in TransferStats::block_deferrals) — consistent with certus-transfer-endpoint's 'completeness path, not a hot path'. Option B: raise to 32 MiB so block_readers=4 is usable at 8 MiB (still capped at pool/8). Either way FR-019 (P5) should state the chosen value and premise.
```

- **Approved:** false

### 10. IDispatcher::read_to_bounce doc: error list — ALIGN

- **Spec location:** `(code doc comment)`
- **Code location:** `components/interfaces/src/idispatcher.rs:893-895; components/dispatcher/src/lib.rs:3995-4000`
- **Severity / confidence:** minor / HIGH
- **Rationale:** Doc-comment change in src (outside this task's write scope; proposal only). The implementation returns NotInitialized and, on an unsettled read, deliberately leaks the lease (dispatcher lib.rs:4040-4048), so 'the lease has been freed' is not always true.

**Current:**

```text
/// Errors: `NoSpace` (the reserve is full now — retry later),
/// `NeverFits` (it never can be satisfied), `Io` (the read failed; the
/// lease has been freed).
```

**Proposed:**

```text
/// Errors: `NoSpace` (the reserve is full now — retry later),
/// `NeverFits` (it never can be satisfied), `Io` (the read failed; the
/// lease has been freed, or leaked if the drive may still write it),
/// `NotInitialized` (the dispatcher or its memory tier is not ready).
```

- **Approved:** false

### 11. ITransferEndpointAdmin configure/initialize error docs — ALIGN

- **Spec location:** `(code doc comment)`
- **Code location:** `components/interfaces/src/itransfer_endpoint.rs:445-451,477-479,659-680; components/certus-transfer-endpoint/src/lib.rs:432-436,506; src/connection/table.rs:267-272`
- **Severity / confidence:** minor / HIGH
- **Rationale:** Implementation refuses block_readers==0 at configure (undocumented there) and send_credits==0 only at initialize via ConnectionTable::new (undocumented anywhere on the interface).

**Current:**

```text
configure: Transport if `message_credits` or `inbound_quota` of zero, or `max_recv_wr` below their sum, or a `deadline_action` that is not implemented. initialize: Transport if the transport cannot listen.
```

**Proposed:**

```text
configure: add 'or `block_readers` of zero'. Either also refuse `send_credits == 0` in configure (consistent placement; code change in certus-transfer-endpoint) or document on initialize: 'Transport if `send_credits` is zero'. Add 'Must be at least 1' to the `send_credits` field doc.
```

- **Approved:** false

### 12. plan.md crate structure + interface count table — HUMAN_DECISION

- **Spec location:** `components/interfaces/specs/001-interfaces/plan.md:8,17-32,42-55`
- **Code location:** `components/interfaces/src/lib.rs; src/{idispatch_map,idispatcher,imemory_tier,itransfer_endpoint}.rs`
- **Severity / confidence:** moderate / HIGH
- **Rationale:** Option A leaves a total and several rows that were already wrong on unstable; Option B is a larger, pre-existing fix. Counts measured by `awk '/define_interface!/{on=1} on' | grep -c '^        fn '`.

**Current:**

```text
"15 interface traits"; `idispatch_map.rs # IDispatchMap trait (16 methods, spdk-gated)`; `imemory_tier.rs # IMemoryTier trait (16 methods, spdk-gated)`; table: IDispatchMap 16 spdk / IDispatcher 16 spdk / IMemoryTier 16 spdk / Total 135; no itransfer_endpoint.rs
```

**Proposed:**

```text
Option A (branch delta only): add `itransfer_endpoint.rs # ITransferEndpoint (6), ITransferEndpointAdmin (3), + types`; rows IDispatchMap 20 (2 integrity-check) none, IDispatcher 24 spdk, IMemoryTier 21 none, ITransferEndpoint 6 none, ITransferEndpointAdmin 3 none. Option B: regenerate the whole table from src (also missing IZyre/IZyreNode/IRemoteLookupRdma*; IRemoteRequestHandler listed but removed), and recompute the total.
```

- **Approved:** false

## Cross-spec note

components/certus-transfer-endpoint/specs/001-rdma-cache-transfer/contracts/itransfer_endpoint.md:378-379 shows bounce_alloc -> Result<*mut u8, BounceError> and bounce_free(ptr, len); shipped code uses BounceLease (interfaces src/imemory_tier.rs:230,234). Belongs to that component's sync.

# Feature Specification: Operational Configuration & Lifecycle

**Feature Branch**: `002-operational-config`
**Created**: 2026-06-18
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> ⚠️ This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## User Scenarios & Testing

### User Story 1 - Auto-Discovery of NVMe Drives (Priority: P1)

An operator starts certus-server with `--drive-count N` instead of listing explicit PCI addresses. The server initializes SPDK, discovers all available NVMe devices (class code 0x010802), prioritizes NUMA-node-0 devices, and selects the first N.

**Why this priority**: Eliminates manual PCI address lookup, which is error-prone and varies per deployment.

**Acceptance Scenarios**:

1. **Given** 4 NVMe drives are available, **When** the operator launches with `--drive-count 2`, **Then** the server selects the first 2 NVMe devices (NUMA-0 preferred) and logs the selected addresses.
2. **Given** only 1 NVMe drive is available, **When** the operator requests `--drive-count 3`, **Then** the server exits with an error indicating insufficient devices.
3. **Given** `--device-pci` is also specified, **When** `--drive-count` is provided, **Then** CLI parsing rejects the conflicting arguments.

---

### User Story 2 - Extent Manager Format vs Recovery (Priority: P1)

An operator controls whether the server formats extent managers (destroying existing data) or recovers previously stored extents on startup, via the `--format` flag.

**Why this priority**: Distinguishes first-time initialization from crash recovery, critical for data durability.

**Acceptance Scenarios**:

1. **Given** existing data on NVMe drives, **When** the server starts without `--format`, **Then** extent managers recover previously stored extents.
2. **Given** existing data on NVMe drives, **When** the server starts with `--format`, **Then** extent managers are reformatted and all previous data is destroyed.

---

### User Story 3 - Memory-Tier Pool Sizing (Priority: P2)

An operator specifies the DRAM memory-tier pool size via `--memory-tier-size` (e.g., `256M`, `2G`). The pool is allocated at startup and registered with CUDA for pinned DMA.

**Why this priority**: Allows tuning memory vs SSD capacity tradeoff per deployment.

**Acceptance Scenarios**:

1. **Given** no `--memory-tier-size` is specified, **When** the server starts, **Then** the pool defaults to 2 GiB.
2. **Given** `--memory-tier-size 512M`, **When** the server starts, **Then** 512 MiB is allocated for the memory tier.
3. **Given** an invalid size string like `abc`, **When** provided to `--memory-tier-size`, **Then** the server exits with a parse error.

---

### User Story 4 - Poller CPU Pinning (Priority: P2)

An operator pins NVMe poller threads to dedicated CPU cores via `--poller-base-cpu N`. Drive i is pinned to core (N + i). This ensures poller threads run on cores in the same NUMA zone as the drives.

**Why this priority**: Critical for achieving maximum IOPS in NUMA-aware deployments.

**Acceptance Scenarios**:

1. **Given** `--poller-base-cpu 2` with 4 drives, **When** the server starts, **Then** poller threads are pinned to cores 2, 3, 4, 5.
2. **Given** no `--poller-base-cpu` is specified, **When** the server starts, **Then** the OS scheduler manages poller thread placement.

---

### User Story 5 - Eviction Tuning (Priority: P3)

An operator tunes the maximum number of eviction attempts before the memory tier returns a pool-full error, via `--max-eviction-attempts`.

**Why this priority**: Allows trading latency for availability under memory pressure.

**Acceptance Scenarios**:

1. **Given** no explicit setting, **When** the server starts, **Then** `max_eviction_attempts` defaults to 2048.
2. **Given** `--max-eviction-attempts 100`, **When** the memory tier is full, **Then** the dispatcher attempts at most 100 evictions before failing with an allocation error.

---

### User Story 6 - Flush to SSD (Priority: P2)

A Python client calls `FlushToSsd` to flush all pending background write-through jobs to SSD and block until complete. This ensures all data is persisted before, e.g., a planned shutdown or checkpoint.

**Why this priority**: Required for data durability guarantees in orchestrated workflows.

**Acceptance Scenarios**:

1. **Given** pending background flush jobs exist, **When** `FlushToSsd` is called, **Then** all jobs complete and the response includes the count of flushed jobs.
2. **Given** no pending jobs, **When** `FlushToSsd` is called, **Then** the response returns `jobs_flushed: 0`.

---

### User Story 7 - Touch with Promotion (Priority: P2)

A Python client calls `Touch` with `promote = true`. The server touches each entry (updating timestamps) and asynchronously promotes SSD-resident entries to the memory tier without GPU DMA.

**Why this priority**: Enables prefetch/warming patterns where the client knows which entries will be needed soon.

**Acceptance Scenarios**:

1. **Given** SSD-resident entries, **When** `Touch` is called with `promote = true`, **Then** the touch succeeds immediately and entries are asynchronously promoted to the memory tier.
2. **Given** entries already in the memory tier, **When** `Touch` is called with `promote = true`, **Then** the touch succeeds and promotion is a no-op.

---

## Requirements

### Functional Requirements

- **FR-001**: System MUST support `--drive-count N` as an alternative to explicit `--device-pci` arguments. The two are mutually exclusive (enforced by CLI parser).
- **FR-002**: When `--drive-count` is used, the system MUST discover NVMe devices via SPDK (PCI class code 0x010802), sort them by NUMA node (node 0 first), and select the first N.
- **FR-003**: If fewer NVMe devices are available than requested by `--drive-count`, the server MUST exit with a descriptive error.
- **FR-004**: System MUST require either `--device-pci` or `--drive-count` to be specified. Omitting both is an error.
- **FR-005**: System MUST support `--format` flag. When present, extent managers are formatted on startup (destructive). When absent, extent managers recover from disk.
- **FR-006**: System MUST support `--memory-tier-size` accepting human-readable sizes (K/M/G suffixes, case-insensitive). Default is 2 GiB.
- **FR-007**: System MUST support `--poller-base-cpu N`. When specified, NVMe poller thread for drive i is pinned to core (N + i).
- **FR-008**: System MUST support `--max-eviction-attempts N` (default 2048). This value is passed to the dispatcher's initialization config.
- **FR-009**: System MUST validate PCI address format (DDDD:BB:DD.F in hex) before passing to the component stack.
- **FR-010**: The gRPC `FlushToSsd` method MUST block until all pending background write-through jobs complete, then return the count of flushed jobs.
- **FR-011**: The gRPC `Touch` method MUST accept a `promote` boolean field. When true, touched entries that are SSD-resident are asynchronously promoted to the memory tier after the touch response is sent.
- **FR-012**: The memory-tier pool MUST be registered with CUDA via `cudaHostRegister` after allocation. If registration fails, the server MUST log a warning and continue (fallback to staged transfer path).
- **FR-013**: The memory-tier pool MUST be allocated on the NUMA node of the first selected NVMe drive. The server resolves the NUMA node by matching the first PCI address against the SPDK device list and passes the node ID to memory-tier initialization. This ensures DRAM and the primary NVMe drive share a NUMA domain for optimal DMA performance.

### CLI Interface

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--device-pci` | String (repeatable) | — | Explicit NVMe PCI addresses |
| `--drive-count` | usize | — | Auto-select first N NVMe drives |
| `--listen` | String | `0.0.0.0:50051` | gRPC listen address |
| `--memory-tier-size` | Size string | `2G` | DRAM pool size |
| `--format` | Flag | false | Format extent managers on startup |
| `--tls-cert` | Path | — | TLS certificate file |
| `--tls-key` | Path | — | TLS private key file |
| `--poller-base-cpu` | usize | — | Base CPU core for poller pinning |
| `--max-eviction-attempts` | usize | 2048 | Max eviction retries |
| `--otel-endpoint` | String | — | OTLP endpoint (requires `otel` feature) |
| `--otel-service-name` | String | `certus-server` | OTel service identity |

## Key Entities

- **PciAddress**: Parsed PCI address with domain, bus, device, function fields.
- **DispatcherConfig**: Configuration struct passed to dispatcher init, containing `data_pci_addrs`, `format_on_init`, `poller_base_cpu`, `max_eviction_attempts`.
- **DmaBuffer**: SPDK-backed DMA-capable memory allocation used by the dispatch map's DMA allocator function.

## Dependencies

- **Spec 001**: gRPC Dispatcher Server (base server and protocol)
- **Spec 003**: OpenTelemetry Observability (metrics export)

## Success Criteria

- **SC-001**: Server starts successfully with `--drive-count` on a multi-NVMe system without specifying PCI addresses.
- **SC-002**: Server recovers extent state across restarts when `--format` is not specified.
- **SC-003**: Poller threads are confirmed pinned to specified cores via `/proc/<pid>/task/*/status`.
- **SC-004**: Memory-tier pool size matches the `--memory-tier-size` argument reported in server logs.

## Implementation Notes

> These notes capture current implementation details that may or may not
> belong in the spec long-term.

- `parse_size()` in main.rs handles K/M/G suffixes with overflow checking.
- `validate_pci_address()` / `parse_pci_address()` validate the DDDD:BB:DD.F format.
- NUMA-aware device sorting uses `sort_by_key(|d| if d.numa_node == 0 { 0 } else { 1 })`.
- The default memory-tier size constant is `2 * 1024 * 1024 * 1024` (2 GiB).
- Promotion in Touch is fire-and-forget via `tokio::task::spawn_blocking`.

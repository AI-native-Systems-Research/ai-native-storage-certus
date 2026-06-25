# Feature Specification: gRPC Dispatcher Server

**Feature Branch**: `dispatcher`  
**Created**: 2026-05-05  
**Updated**: 2026-05-29  
**Status**: Draft  
**Input**: User description: "Build an application that exposes an instance of the dispatcher component, and its interface IDispatcher, to a Python client via gRPC. The gRPC protocol should expose the methods on the IDispatcher interface. Configuration parameters, such as PCI addresses for data NVMe devices, should be command line configurable. The implementation must include a Python test-client that provides basic testing. The protocol should support multi-instance list-based parameters."

## User Scenarios & Testing

### User Story 1 - Start Dispatcher Server with Device Configuration (Priority: P1)

An operator starts the certus-server application, providing one or more PCI addresses for data NVMe devices via command-line arguments. The server initializes the dispatcher component stack (including GPU services, dispatch map, and memory-tier), starts a gRPC endpoint, and is ready to accept client connections.

**Why this priority**: Without a running, properly configured server, no client operations are possible. This is the foundation for all other functionality.

**Independent Test**: Can be tested by launching the server with valid PCI addresses and confirming it reports readiness on the configured port.

**Acceptance Scenarios**:

1. **Given** the server binary is available, **When** the operator launches it with `--device-pci 0000:02:00.0 --device-pci 0000:03:00.0`, **Then** the server initializes the dispatcher and listens for gRPC connections on the configured port.
2. **Given** the server is launched without required arguments, **When** no `--device-pci` address is provided, **Then** the server exits with a clear usage error message.
3. **Given** the server is running, **When** the operator sends a termination signal (SIGTERM/SIGINT), **Then** the server gracefully shuts down the dispatcher and exits cleanly.

---

### User Story 2 - Python Client Populates Cache Entries in Batch (Priority: P1)

A Python client connects to the running server and submits a batch of (key, ipc_handle) pairs in a single `populate` call. The server iterates the list and calls the underlying dispatcher `populate()` for each pair, returning per-entry success/failure status without requiring one round-trip per entry.

**Why this priority**: Batch population is the primary write path and demonstrates the core multi-instance list parameter design that avoids excessive round-trips.

**Independent Test**: Can be tested by running the Python test client against a running server, submitting a batch of entries, and verifying each entry is acknowledged.

**Acceptance Scenarios**:

1. **Given** the server is running and initialized, **When** a Python client calls `populate([(key0, ipc0), (key1, ipc1), ...])`, **Then** the server calls the dispatcher's `populate()` for each pair and returns a list of per-entry results (success or error).
2. **Given** one entry in a batch already exists, **When** the batch populate is called, **Then** the existing-key entry reports `AlreadyExists` while other entries in the batch succeed independently.
3. **Given** the server is not yet initialized, **When** populate is called, **Then** the client receives a clear "not initialized" error.

---

### User Story 3 - Python Client Performs Batch Lookup and Check (Priority: P2)

A Python client submits a batch of cache keys to `lookup` or `check`. The server iterates the list, calling the dispatcher's respective method for each key, and returns results for all keys in a single response.

**Why this priority**: Read operations (lookup and check) are the primary retrieval path and complete the CRUD lifecycle needed for a functional system.

**Independent Test**: Can be tested by populating entries first, then batch-looking them up and verifying data integrity.

**Acceptance Scenarios**:

1. **Given** entries with keys 1, 2, 3 exist, **When** a Python client calls `lookup([(1, ipc_a), (2, ipc_b), (3, ipc_c)])`, **Then** each ipc_handle receives the cached data and the response indicates per-key success.
2. **Given** entries with keys 1, 2 exist but key 99 does not, **When** check is called with keys `[1, 2, 99]`, **Then** the response returns `[true, true, false]` respectively.
3. **Given** a key does not exist, **When** lookup is called for that key, **Then** the per-entry result reports `KeyNotFound`.

---

### User Story 4 - Python Client Removes Cache Entries in Batch (Priority: P2)

A Python client submits a batch of cache keys to remove. The server calls the dispatcher's `remove()` for each key and returns per-entry results.

**Why this priority**: Removal completes the cache lifecycle but is lower priority than population and retrieval.

**Independent Test**: Can be tested by populating entries, removing them in batch, then verifying they no longer exist via check.

**Acceptance Scenarios**:

1. **Given** entries with keys 1, 2, 3 exist, **When** the client calls `remove([1, 2, 3])`, **Then** all entries are removed and per-entry success is returned.
2. **Given** key 99 does not exist, **When** remove is called with `[99]`, **Then** the per-entry result reports `KeyNotFound`.

---

### User Story 5 - Python Client Touches Cache Entries in Batch (Priority: P3)

A Python client submits a batch of cache keys to `touch`. The server calls the dispatcher's `touch()` for each key and returns per-entry results, refreshing the cache entry's access timestamp or priority without transferring data.

**Why this priority**: Touch supports cache management policies (e.g., LRU refresh) but is not required for basic cache functionality.

**Independent Test**: Can be tested by populating entries, touching them, and verifying success/failure for existing and non-existing keys.

**Acceptance Scenarios**:

1. **Given** entries with keys 1, 2, 3 exist, **When** the client calls `touch([1, 2, 3])`, **Then** all entries are touched and per-entry success is returned.
2. **Given** key 99 does not exist, **When** touch is called with `[99]`, **Then** the per-entry result reports `KeyNotFound`.

---


### Edge Cases

- Batch requests with duplicate keys are rejected entirely via pre-validation (FR-015).
- How does the system handle a client disconnection mid-batch?
- What happens when the IPC handle data size is zero or exceeds device capacity?
- How does the server behave when all data drives are exhausted (allocation failure)?
- What happens if the server receives a request during shutdown?

## Requirements

### Functional Requirements

- **FR-001**: System MUST expose a gRPC service with methods mapping to IDispatcher data operations: `lookup`, `check`, `remove`, `populate`, `touch`, and `clear_memory_tier`. Lifecycle operations (initialize/shutdown) are NOT exposed via gRPC.
- **FR-002**: The `populate` gRPC method MUST accept a list of (key, ipc_handle) pairs and execute the dispatcher's `populate()` for each pair server-side, returning per-entry results.
- **FR-003**: The `lookup` gRPC method MUST accept a list of (key, ipc_handle) pairs and execute the dispatcher's `batch_lookup()` to process all entries in a single call, returning per-entry results. The batch call enables parallel cold promotion across drives and multi-queue threads for significantly higher throughput than sequential per-entry `lookup()`. Within a single batch, IPC handles that share the same CUDA IPC memory handle are opened once and reused across entries to reduce open/close overhead.
- **FR-004**: The `check` gRPC method MUST accept a list of cache keys and execute the dispatcher's `check()` for each key, returning a list of boolean results.
- **FR-005**: The `remove` gRPC method MUST accept a list of cache keys and execute the dispatcher's `remove()` for each key, returning per-entry results.
- **FR-005b**: The `touch` gRPC method MUST accept a list of cache keys and execute the dispatcher's `touch()` for each key, returning per-entry results.
- **FR-006**: The server application MUST accept command-line arguments for: one or more data NVMe PCI addresses (`--device-pci`, repeatable), gRPC listen address/port (`--listen`), and optional TLS certificate/key paths (`--tls-cert`, `--tls-key`).
- **FR-007**: Per-entry results MUST include the original key and either a success indicator or an error code with a descriptive message, so the client can correlate results to its input. Batch processing uses a partial-success model: each entry is processed independently regardless of other entries' success or failure.
- **FR-008**: The server MUST auto-initialize the dispatcher on startup using the PCI addresses provided via command-line arguments. The component initialization stack is: SPDK environment init, GPU services init, dispatch map init (ephemeral, no persistence), memory-tier init with CUDA host registration, then dispatcher init with data PCI addresses.
- **FR-008b**: The dispatch map MUST start fresh on each server launch (no on-disk persistence). There is no extent manager; the dispatch map is initialized without persistent storage backing.
- **FR-008c**: The memory-tier pool MUST be registered with CUDA via `cudaHostRegister` after initialization to enable pinned DMA transfers between GPU and the memory tier. If registration fails, the server logs a warning and continues (staged transfer path is used as fallback).
- **FR-009**: The server MUST shut down the dispatcher when receiving SIGTERM/SIGINT, draining active requests and completing all in-flight operations before exiting.
- **FR-010**: A Python test client MUST be provided that exercises all gRPC methods, demonstrating batch operations and error handling.
- **FR-011**: The IPC handle in the gRPC protocol MUST be represented as a serializable structure containing: (1) a CUDA IPC memory handle (`bytes`, 64-byte opaque blob from `cudaIpcGetMemHandle`), (2) a size field (`uint32`, data size in bytes), and (3) a `gpu_device_id` field (`int32`, the CUDA device ordinal that allocated the memory). The client MUST populate `gpu_device_id` so the server can call `cudaSetDevice` on the correct device before opening the handle. The server opens the IPC handle via `cudaIpcOpenMemHandle` to obtain a device pointer in its own CUDA context; the device pointer is cached for the lifetime of the request batch (see FR-018). Handles are released via `cudaIpcCloseMemHandle` when no longer needed.
- **FR-013**: The server MUST accept multiple client connections but serialize request processing (one request at a time). Concurrent requests are queued, not rejected. Serialization is achieved via a Mutex around the dispatcher reference; gRPC request handlers use `spawn_blocking` to avoid blocking the async runtime.
- **FR-014**: The server MUST support optional TLS encryption via CLI flags (`--tls-cert`, `--tls-key`). When both are provided, TLS is enabled. When not provided, the server runs in plaintext mode.
- **FR-015**: The server MUST pre-validate batch requests for duplicate keys. If a batch contains the same key more than once, the entire batch MUST be rejected with an error identifying the duplicate key(s).
- **FR-016**: The server MUST support multiple data NVMe devices. The `--device-pci` CLI argument is repeatable, and all provided PCI addresses are passed to the dispatcher via `DispatcherConfig.data_pci_addrs` for multi-SSD striping/distribution.
- **FR-017**: The server MUST expose a `ClearMemoryTier` gRPC method that accepts an empty request and calls the dispatcher's `clear_memory_tier()` method. The response MUST include the number of entries cleared (`entries_cleared: uint64`). This operation evicts all entries from the server's DRAM memory-tier, demoting them to SSD-backed state in the dispatch map.
- **FR-018**: The server MUST maintain a global, process-lifetime IPC handle cache (keyed by the 64-byte CUDA IPC handle bytes) on the `DispatcherService` instance. Each cache entry stores the opened device pointer (`dev_ptr`), the `gpu_device_id` used when opening, and a reference count. When a batch operation requires an IPC handle: if the handle is already in the cache the existing device pointer is reused and the reference count is incremented; if the handle is not cached, the server opens it via `cudaIpcOpenMemHandle` (after calling `cudaSetDevice`, per FR-019), inserts it with `refcount = 1`, and records the opened pointer. After processing a batch, the server decrements the reference count for each handle opened during that batch; when the reference count reaches zero, the server calls `cudaIpcCloseMemHandle` and removes the entry from the cache. This design eliminates "resource already mapped" errors from concurrent batches and removes serialization on CUDA's global IPC lock.
- **FR-019**: Before calling `cudaIpcOpenMemHandle` for a handle that is not already in the IPC cache, the server MUST call `cudaSetDevice(gpu_device_id)` when the `gpu_device_id` field of the `IpcHandle` message is non-negative (>= 0). If `cudaSetDevice` fails, the server MUST propagate the error as an `IoError` for that entry without opening the handle. This ensures the server CUDA context is associated with the correct device before the handle is opened, which is required for correct operation on multi-GPU systems.

## Implementation Details

### FR-010: Python Test Client

**Location**: `apps/certus-server/python-client/test_client.py` (548 lines)

**Status**: ✅ Production-ready

**Prerequisites**:
- Python 3.8+
- `grpcio` and `grpcio-tools` packages
- PyTorch or CUDA toolkit (for GPU memory and IPC handle generation)
- Running certus-server instance

**CLI Options**:
```
test_client.py [--server ADDRESS:PORT] [--skip-tests] [--benchmark]
  --server       gRPC server address (default: localhost:50051)
  --skip-tests   Skip functional tests, run benchmark only
  --benchmark    Run performance benchmarks (throughput/latency measurement)
```

**Functional Test Suite** (9 test cases):
1. **test_populate**: Batch populate with 100 entries, verify per-entry success
2. **test_populate_duplicate_key**: Reject batch with duplicate keys
3. **test_populate_already_exists**: Handle AlreadyExists error for duplicate key in second batch
4. **test_check**: Verify check returns correct boolean for existing/missing keys
5. **test_lookup**: Retrieve entries and verify data integrity
6. **test_lookup_not_found**: Handle KeyNotFound error for missing keys
7. **test_remove**: Batch remove and verify entries no longer exist
8. **test_remove_not_found**: Handle KeyNotFound error during removal
9. **test_touch**: Touch entries and verify success

**Benchmark Suite**:
- **Memory-Tier Lookup Latency**: Measure lookup latency for hot (in-memory) entries
- **SSD-Tier Lookup Latency**: Measure lookup latency for cold (SSD-backed) entries
- **Throughput**: Measure sustained lookup throughput (GB/s) for mixed workload
- **Batch Scaling**: Measure throughput vs batch size (10, 100, 1000+ entries)

**Benchmark Output**:
```
Memory-tier lookup: 12.34 µs/op (avg of 10000 ops)
SSD-tier lookup: 234.56 µs/op (avg of 1000 ops)
Throughput: 456.78 GB/s
```

**Exit Codes**:
- `0`: All tests passed
- `1`: Functional test failed
- `2`: Benchmark failed
- `3`: Connection error

**Example Invocations**:
```bash
# Run full test suite and benchmark
python3 test_client.py --server localhost:50051

# Benchmark only (skip functional tests)
python3 test_client.py --skip-tests --benchmark

# Run against remote server
python3 test_client.py --server remote-host:50051
```

### FR-014: TLS Support

**Location**: `apps/certus-server/src/main.rs` (lines 50-56, 300-308)

**Status**: ✅ Production-ready

**CLI Flags**:
- `--tls-cert <PATH>`: Path to PEM-encoded certificate file (required if TLS enabled)
- `--tls-key <PATH>`: Path to PEM-encoded private key file (required if TLS enabled)

**Behavior**:
- TLS is **enabled** if both `--tls-cert` and `--tls-key` are provided
- TLS is **disabled** (plaintext mode) if either or both flags are omitted
- Both flags must be present to enable TLS; partial specification is an error

**Certificate Requirements**:
- PEM-encoded X.509 certificate (`.pem` or `.crt` file)
- PEM-encoded RSA/ECDSA private key (`.key` or `.pem` file)
- Certificate must be valid for the server's hostname/IP

**Certificate Generation** (OpenSSL):
```bash
# Generate self-signed certificate (for testing)
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365 -nodes

# Generate certificate signing request (for production)
openssl req -newkey rsa:4096 -keyout key.pem -out cert.csr
# Then sign with your CA
```

**Python Client Configuration** (with TLS):
```python
import grpc

# Load client certificate (if mutual TLS required)
with open('client-cert.pem', 'rb') as f:
    client_cert_chain = f.read()
with open('client-key.pem', 'rb') as f:
    client_private_key = f.read()

# Load server CA certificate (or use default system CA bundle)
with open('server-cert.pem', 'rb') as f:
    root_certificates = f.read()

credentials = grpc.ssl_channel_credentials(
    root_certificates=root_certificates,
    private_key=client_private_key,
    certificate_chain=client_cert_chain
)

channel = grpc.secure_channel('localhost:50051', credentials)
```

**Server Logging**:
- INFO level: "certus-server: TLS enabled" when TLS is active
- ERROR level: Certificate/key file not found or invalid

**Example Invocations**:
```bash
# Plaintext mode (default)
./certus-server --device-pci 0000:02:00.0

# TLS enabled with self-signed certificate
./certus-server --device-pci 0000:02:00.0 --tls-cert cert.pem --tls-key key.pem

# TLS with custom port
./certus-server --device-pci 0000:02:00.0 --listen 0.0.0.0:50051 --tls-cert cert.pem --tls-key key.pem
```

**Security Notes**:
- Self-signed certificates are suitable for development and testing only
- For production, use certificates signed by a trusted CA
- Consider mutual TLS (mTLS) for client authentication in sensitive environments
- Ensure private key files have restricted permissions (e.g., `chmod 600`)

### Key Entities

- **DispatcherConfig**: Configuration containing a list of data PCI addresses (`data_pci_addrs: Vec<String>`).
- **CacheKey**: A 64-bit unsigned integer identifying a cache entry.
- **IpcHandle**: Opaque handle to client GPU memory containing: a 64-byte CUDA IPC memory handle (`bytes`), a data size (`uint32`), and the CUDA device ordinal that allocated the memory (`gpu_device_id: int32`). The `gpu_device_id` field is used by the server to call `cudaSetDevice` before opening the IPC handle (see FR-019).
- **EntryResult**: Per-entry outcome containing the original key, success/failure status, and optional error information.
- **CheckResult**: Per-entry outcome for check operations containing the key and a boolean `exists` field.
- **BatchRequest**: A list of operation parameters (key + optional ipc_handle) sent in a single gRPC call.
- **BatchResponse**: A list of per-entry results returned from a single gRPC call.

## Component Stack

The server initializes components in the following order:

1. **SPDK Environment** — Initializes the SPDK userspace framework for direct NVMe access.
2. **Logger** — Shared logging component connected to all other components.
3. **GPU Services** — Initializes CUDA context for GPU memory operations.
4. **Dispatch Map** — Ephemeral in-memory mapping (starts fresh each launch, no extent manager, no persistence).
5. **Memory Tier** — Pool allocator for staging data; pool is registered with CUDA via `cudaHostRegister` for pinned DMA.
6. **Dispatcher** — Top-level orchestrator bound to dispatch map, memory tier, GPU services, SPDK env, and logger. Initialized with `DispatcherConfig { data_pci_addrs }`.

## Success Criteria

### Measurable Outcomes

- **SC-001**: A Python client can populate, check, lookup, touch, and remove 100 cache entries in batch with a single round-trip per operation type (5 total round-trips for the full lifecycle).
- **SC-002**: Batch operations of 1000 entries complete without timeout or connection errors.
- **SC-003**: Per-entry error reporting allows the client to identify exactly which entries in a batch failed and why.
- **SC-004**: The server starts and becomes ready to accept connections within 10 seconds of launch (excluding SPDK initialization time).
- **SC-005**: The Python test client provides pass/fail results for all defined acceptance scenarios.
- **SC-006**: The server correctly initializes with multiple `--device-pci` arguments and distributes operations across all configured data devices.

## Clarifications

### Session 2026-05-05

- Q: Does the server auto-initialize on startup or require client to call initialize()? → A: Server auto-initializes on startup using CLI args. gRPC protocol does NOT include initialize/shutdown methods; lifecycle is managed via process start and SIGTERM/SIGINT only.
- Q: When one entry in a batch fails, does the server continue or abort? → A: Continue processing all entries independently; each entry gets its own success/failure result (partial success model).
- Q: Can multiple clients connect simultaneously? → A: Multiple connections allowed but requests are serialized (one-at-a-time processing).
- Q: Should the gRPC endpoint use authentication or encryption? → A: Optional TLS support via CLI flag (off by default); no authentication layer.
- Q: What happens when a batch contains duplicate keys? → A: Reject entire batch with an error if duplicate keys are detected (pre-validation before processing).

### Session 2026-05-22

- Q: Is a metadata device required? → A: No. The `--metadata-pci` argument has been removed. The dispatch map operates ephemerally without persistent storage. Only data NVMe devices are configured.
- Q: Does the dispatch map persist state across restarts? → A: No. The dispatch map starts fresh on each server launch. There is no extent manager or on-disk persistence for the mapping table.
- Q: How is GPU DMA performance optimized? → A: The memory-tier pool is registered with CUDA via `cudaHostRegister` after initialization, enabling pinned (zero-copy) DMA transfers. If registration fails, the server falls back to a staged transfer path.

### Session 2026-05-29

- Q: How does the server avoid "resource already mapped" errors when multiple concurrent batches reference the same CUDA IPC handle? → A: A global persistent IPC handle cache on `DispatcherService` keyed by the 64-byte handle bytes deduplicates open/close calls. Concurrent batches sharing a handle increment/decrement a reference count; `cudaIpcCloseMemHandle` is only called when the count reaches zero. This removes serialization on CUDA's global IPC lock and eliminates the "resource already mapped" error class entirely.
- Q: When must the server call `cudaSetDevice`? → A: Immediately before calling `cudaIpcOpenMemHandle` for any handle not already in the cache, using the `gpu_device_id` provided in the `IpcHandle` message. Required for multi-GPU correctness; skipped when `gpu_device_id < 0` (sentinel value meaning "not specified").
- Q: Does the IpcHandle proto message need a device identifier? → A: Yes. Field 3 (`gpu_device_id: int32`) was added to carry the CUDA device ordinal from the client so the server can call `cudaSetDevice` correctly before opening the handle.

## Assumptions

- The Python client and the server run on the same machine (or same NUMA domain) since IPC handles reference GPU memory addresses accessible via local DMA.
- The SPDK environment is pre-configured (hugepages, IOMMU, device unbinding) before the server starts.
- A single dispatcher instance per server process is sufficient (no multi-tenancy required).
- The gRPC listen port defaults to 50051 but is overridable via command-line argument (`--listen`).
- The IPC handle uses CUDA's cross-process memory sharing (`cudaIpcGetMemHandle`/`cudaIpcOpenMemHandle`). Client and server may be separate processes; they share GPU memory via the CUDA IPC mechanism rather than raw pointer values.
- The Python test client uses the standard `grpcio` library.
- Error codes in the gRPC protocol map directly to `DispatcherError` variants (NotInitialized, KeyNotFound, AlreadyExists, AllocationFailed, IoError, Timeout, InvalidParameter).
- Cache state is ephemeral: all entries are lost on server restart since the dispatch map has no persistence backing.

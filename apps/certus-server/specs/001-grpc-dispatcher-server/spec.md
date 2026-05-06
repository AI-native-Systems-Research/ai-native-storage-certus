# Feature Specification: gRPC Dispatcher Server

**Feature Branch**: `dispatcher`  
**Created**: 2026-05-05  
**Status**: Draft  
**Input**: User description: "Build an application that exposes an instance of the dispatcher component, and its interface IDispatcher, to a Python client via gRPC. The gRPC protocol should expose the methods on the IDispatcher interface. Configuration parameters, such as PCI address for metadata and data NVMe devices, should be command line configurable. The implementation must include a Python test-client that provides basic testing. The protocol should support multi-instance list-based parameters."

## User Scenarios & Testing

### User Story 1 - Start Dispatcher Server with Device Configuration (Priority: P1)

An operator starts the certus-server application, providing PCI addresses for the metadata NVMe device and one or more data NVMe devices via command-line arguments. The server initializes the dispatcher component, starts a gRPC endpoint, and is ready to accept client connections.

**Why this priority**: Without a running, properly configured server, no client operations are possible. This is the foundation for all other functionality.

**Independent Test**: Can be tested by launching the server with valid PCI addresses and confirming it reports readiness on the configured port.

**Acceptance Scenarios**:

1. **Given** the server binary is available, **When** the operator launches it with `--metadata-pci 0000:01:00.0 --data-pci 0000:02:00.0 --data-pci 0000:03:00.0`, **Then** the server initializes the dispatcher and listens for gRPC connections on the configured port.
2. **Given** the server is launched without required arguments, **When** a required PCI address is missing, **Then** the server exits with a clear usage error message.
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


### Edge Cases

- Batch requests with duplicate keys are rejected entirely via pre-validation (FR-015).
- How does the system handle a client disconnection mid-batch?
- What happens when the IPC handle data size is zero or exceeds device capacity?
- How does the server behave when all data drives are exhausted (allocation failure)?
- What happens if the server receives a request during shutdown?

## Requirements

### Functional Requirements

- **FR-001**: System MUST expose a gRPC service with methods mapping to IDispatcher data operations: `lookup`, `check`, `remove`, and `populate`. Lifecycle operations (initialize/shutdown) are NOT exposed via gRPC.
- **FR-002**: The `populate` gRPC method MUST accept a list of (key, ipc_handle) pairs and execute the dispatcher's `populate()` for each pair server-side, returning per-entry results.
- **FR-003**: The `lookup` gRPC method MUST accept a list of (key, ipc_handle) pairs and execute the dispatcher's `lookup()` for each pair server-side, returning per-entry results.
- **FR-004**: The `check` gRPC method MUST accept a list of cache keys and execute the dispatcher's `check()` for each key, returning a list of boolean results.
- **FR-005**: The `remove` gRPC method MUST accept a list of cache keys and execute the dispatcher's `remove()` for each key, returning per-entry results.
- **FR-006**: The server application MUST accept command-line arguments for: metadata NVMe PCI address, one or more data NVMe PCI addresses, gRPC listen address/port, and optional TLS certificate/key paths.
- **FR-014**: The server MUST support optional TLS encryption via CLI flags (`--tls-cert`, `--tls-key`). When not provided, the server runs in plaintext mode.
- **FR-007**: Per-entry results MUST include the original key and either a success indicator or an error code with a descriptive message, so the client can correlate results to its input. Batch processing uses a partial-success model: each entry is processed independently regardless of other entries' success or failure.
- **FR-008**: The server MUST auto-initialize the dispatcher on startup using the PCI addresses provided via command-line arguments.
- **FR-009**: The server MUST shut down the dispatcher when receiving SIGTERM/SIGINT, draining active requests and completing all in-flight operations before exiting.
- **FR-010**: A Python test client MUST be provided that exercises all gRPC methods, demonstrating batch operations and error handling.
- **FR-011**: The IPC handle in the gRPC protocol MUST be represented as a serializable structure containing a CUDA IPC memory handle (64-byte opaque blob from `cudaIpcGetMemHandle`) and a size field (`uint32`, data size in bytes). The server opens the IPC handle via `cudaIpcOpenMemHandle` to obtain a device pointer in its own CUDA context, performs the operation, then closes the handle via `cudaIpcCloseMemHandle`.
- **FR-013**: The server MUST accept multiple client connections but serialize request processing (one request at a time). Concurrent requests are queued, not rejected.
- **FR-015**: The server MUST pre-validate batch requests for duplicate keys. If a batch contains the same key more than once, the entire batch MUST be rejected with an error identifying the duplicate key(s).

### Key Entities

- **DispatcherConfig**: Configuration containing metadata PCI address and a list of data PCI addresses.
- **CacheKey**: A 64-bit unsigned integer identifying a cache entry.
- **IpcHandle**: Opaque handle to client GPU memory containing a 64-byte CUDA IPC memory handle and a size (uint32).
- **EntryResult**: Per-entry outcome containing the original key, success/failure status, and optional error information.
- **BatchRequest**: A list of operation parameters (key + optional ipc_handle) sent in a single gRPC call.
- **BatchResponse**: A list of per-entry results returned from a single gRPC call.

## Success Criteria

### Measurable Outcomes

- **SC-001**: A Python client can populate, check, lookup, and remove 100 cache entries in batch with a single round-trip per operation type (4 total round-trips for the full lifecycle).
- **SC-002**: Batch operations of 1000 entries complete without timeout or connection errors.
- **SC-003**: Per-entry error reporting allows the client to identify exactly which entries in a batch failed and why.
- **SC-004**: The server starts and becomes ready to accept connections within 10 seconds of launch (excluding SPDK initialization time).
- **SC-005**: The Python test client provides pass/fail results for all defined acceptance scenarios.

## Clarifications

### Session 2026-05-05

- Q: Does the server auto-initialize on startup or require client to call initialize()? → A: Server auto-initializes on startup using CLI args. gRPC protocol does NOT include initialize/shutdown methods; lifecycle is managed via process start and SIGTERM/SIGINT only.
- Q: When one entry in a batch fails, does the server continue or abort? → A: Continue processing all entries independently; each entry gets its own success/failure result (partial success model).
- Q: Can multiple clients connect simultaneously? → A: Multiple connections allowed but requests are serialized (one-at-a-time processing).
- Q: Should the gRPC endpoint use authentication or encryption? → A: Optional TLS support via CLI flag (off by default); no authentication layer.
- Q: What happens when a batch contains duplicate keys? → A: Reject entire batch with an error if duplicate keys are detected (pre-validation before processing).

## Assumptions

- The Python client and the server run on the same machine (or same NUMA domain) since IPC handles reference GPU memory addresses accessible via local DMA.
- The SPDK environment is pre-configured (hugepages, IOMMU, device unbinding) before the server starts.
- A single dispatcher instance per server process is sufficient (no multi-tenancy required).
- The gRPC listen port defaults to 50051 but is overridable via command-line argument.
- The IPC handle uses CUDA's cross-process memory sharing (`cudaIpcGetMemHandle`/`cudaIpcOpenMemHandle`). Client and server may be separate processes; they share GPU memory via the CUDA IPC mechanism rather than raw pointer values.
- The Python test client uses the standard `grpcio` library.
- Error codes in the gRPC protocol map directly to `DispatcherError` variants (NotInitialized, KeyNotFound, AlreadyExists, AllocationFailed, IoError, Timeout, InvalidParameter).

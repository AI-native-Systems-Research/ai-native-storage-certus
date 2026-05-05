# Data Model: gRPC Dispatcher Server

## Entities

### CacheKey
- **Type**: `uint64`
- **Identity**: Value itself is the unique identifier
- **Source**: Provided by client in all operations

### IpcHandle
- **Fields**:
  - `address`: `uint64` — GPU memory pointer (valid in shared address space)
  - `size`: `uint32` — data size in bytes
- **Constraints**: size > 0 (validated server-side; zero-size rejected with InvalidParameter)
- **Lifecycle**: Ephemeral; valid only for the duration of a single request

### EntryResult
- **Fields**:
  - `key`: `uint64` — echoed from request for correlation
  - `success`: `bool` — whether this entry's operation succeeded
  - `error_code`: `enum` (optional) — one of: KEY_NOT_FOUND, ALREADY_EXISTS, ALLOCATION_FAILED, IO_ERROR, TIMEOUT, INVALID_PARAMETER
  - `error_message`: `string` (optional) — human-readable description

### PopulateEntry
- **Fields**:
  - `key`: `uint64`
  - `ipc_handle`: IpcHandle
- **Used in**: BatchPopulateRequest

### LookupEntry
- **Fields**:
  - `key`: `uint64`
  - `ipc_handle`: IpcHandle
- **Used in**: BatchLookupRequest

### CheckResult
- **Fields**:
  - `key`: `uint64`
  - `exists`: `bool`
- **Used in**: BatchCheckResponse

## State Transitions

```
Server Start → [CLI args parsed] → [SPDK init] → [Component wiring] → [Dispatcher initialized] → [gRPC listening]
                                                                                                        ↓
                                                                                               [Accepting requests]
                                                                                                        ↓
                                                                                               [SIGTERM/SIGINT]
                                                                                                        ↓
                                                                                               [Drain active request]
                                                                                                        ↓
                                                                                               [Dispatcher shutdown]
                                                                                                        ↓
                                                                                               [Process exit]
```

## Validation Rules

- **Batch duplicate check**: Before processing any entry in a batch, scan all keys for duplicates. If found, reject the entire batch (FR-015).
- **IPC handle size**: Must be > 0; zero-size handles are rejected per-entry.
- **PCI address format**: Must match `DDDD:BB:DD.F` hex format (validated at startup).
- **Batch size**: No explicit limit; bounded by gRPC message size (default 4MB, configurable).

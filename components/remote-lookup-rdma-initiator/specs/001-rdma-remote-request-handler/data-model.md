# Data Model: RDMA Remote Lookup Initiator

## Entities

### Session

Represents an active RDMA connection from a remote Certus node.

| Field | Type | Description |
|-------|------|-------------|
| id | u64 | Unique session identifier (monotonically increasing) |
| state | SessionState | Current lifecycle state |
| remote_addr | SocketAddr | Remote node address (from rdma_cm) |
| protocol_version | u32 | Negotiated protocol version from handshake |
| created_at | Instant | Connection establishment time |
| batches_processed | u64 | Count of batches handled on this session |

**State transitions**:
```
Connecting → Handshake → Active → Closing → Closed
     │            │          │
     └────────────┴──────────┴──→ Closed (on CM disconnect event)
```

### LookupBatchRequest

A batch of cache lookups submitted by a remote node over an active session.

| Field | Type | Description |
|-------|------|-------------|
| batch_id | u32 | Caller-assigned identifier for correlation |
| entries | Vec<LookupEntry> | 1–64 lookup entries |

### LookupEntry

A single lookup request within a batch.

| Field | Type | Description |
|-------|------|-------------|
| cache_key | u64 (CacheKey) | 64-bit identifier for the cached object |
| remote_addr | u64 | Remote memory address for result delivery |
| rkey | u32 | 32-bit RDMA memory key authorizing write to remote_addr |
| max_size | u32 | Maximum bytes the caller has allocated at remote_addr |

### LookupBatchResponse

Response returned to caller after batch processing completes.

| Field | Type | Description |
|-------|------|-------------|
| batch_id | u32 | Echoed from request for correlation |
| results | Vec<EntryResult> | One result per entry, same order as request |

### EntryResult

Per-entry result within a batch response.

| Field | Type | Description |
|-------|------|-------------|
| cache_key | u64 | Echoed key for correlation |
| success | bool | Whether lookup and RDMA write succeeded |
| bytes_written | u32 | Number of bytes written to remote memory (0 on failure) |
| error_code | ErrorCode | Error type if success is false |
| error_message | string | Human-readable error detail (optional) |

### HandshakeRequest

Sent by caller immediately after connection establishment.

| Field | Type | Description |
|-------|------|-------------|
| protocol_version | u32 | Caller's protocol version |
| client_id | string | Optional identifier for logging/telemetry |

### HandshakeResponse

Sent by handler in response to handshake.

| Field | Type | Description |
|-------|------|-------------|
| accepted | bool | Whether version is compatible |
| server_version | u32 | Handler's protocol version |
| max_batch_size | u32 | Maximum entries per batch (64) |
| error_message | string | Reason for rejection (if not accepted) |

### CloseRequest

Sent by caller to initiate graceful session teardown.

| Field | Type | Description |
|-------|------|-------------|
| reason | string | Optional close reason for logging |

### CloseResponse

Acknowledgement of close request.

| Field | Type | Description |
|-------|------|-------------|
| sessions_batches_total | u64 | Total batches processed during this session |

## Enumerations

### SessionState

```
Connecting    — CM connection event received, not yet handshaked
Handshake     — Waiting for/processing handshake message
Active        — Ready to process lookup batches
Closing       — Close requested, draining in-flight operations
Closed        — All resources released
```

### ErrorCode

```
Unspecified       — Default/unknown error
KeyNotFound       — CacheKey not present in local cache
DispatchError     — IDispatcher returned an error
InvalidRequest    — Malformed message or batch too large
RdmaWriteFailed   — RDMA Write to remote memory failed
NotInitialized    — Handler missing required receptacles
VersionMismatch   — Protocol version incompatible (handshake only)
```

## Relationships

```
Session 1──* LookupBatchRequest    (a session processes many batches)
LookupBatchRequest 1──* LookupEntry  (a batch contains 1-64 entries)
LookupBatchResponse 1──* EntryResult (one result per entry)
Session 1──1 HandshakeRequest      (exactly one handshake per session)
```

## Validation Rules

- Batch size: 1 ≤ entries.len() ≤ 64
- CacheKey: any valid u64 value
- rkey: any valid u32 (trusted within isolated fabric)
- remote_addr: any valid u64 (trusted within isolated fabric)
- max_size: must be > 0
- protocol_version in handshake: must exactly match handler's version (no negotiation)

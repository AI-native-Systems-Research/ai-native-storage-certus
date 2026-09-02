# Data Model: Block Device Filesys Component

**Date**: 2026-06-04

## Entities

### BlockDeviceFilesysComponent

The top-level component struct produced by `define_component!`.

| Field | Type | Description |
|-------|------|-------------|
| file_path | `RwLock<Option<PathBuf>>` | Path to the backing file |
| block_size | `AtomicU32` | Block/sector size in bytes (caller-supplied; enforced minimum 512, no implicit default) |
| num_blocks | `AtomicU64` | Total number of blocks (device capacity) |
| actor_handle | `Mutex<Option<ActorHandle<ControlMessage>>>` | Handle to the actor thread |
| next_client_id | `AtomicU64` | Monotonically increasing client ID counter |
| telemetry_stats | `Mutex<Option<Arc<dyn Any + Send + Sync>>>` | Type-erased telemetry (feature-gated) |

**Provides**: `[IBlockDevice]`
**Receptacles**: `{ logger: ILogger }`

### DeviceConfig

Validated configuration passed to the actor at initialization.

| Field | Type | Description |
|-------|------|-------------|
| file_path | `PathBuf` | Validated path to backing file |
| block_size | `u32` | Sector/block size in bytes |
| num_blocks | `u64` | Total block count |
| total_bytes | `u64` | block_size × num_blocks (pre-computed) |

**Invariants**:
- `block_size` must be a power of 2, minimum 512
- `num_blocks` must be > 0
- `total_bytes` must not overflow u64

### FilesysActor (ActorHandler<ControlMessage>)

The actor state, owned by the actor thread.

| Field | Type | Description |
|-------|------|-------------|
| fd | `OwnedFd` | File descriptor for backing file |
| config | `DeviceConfig` | Device geometry |
| ring | `IoUring` | io_uring instance for async ops |
| clients | `HashMap<u64, ClientSession>` | Connected clients by ID |
| inflight | `HashMap<u64, InflightOp>` | In-flight async ops by OpHandle |
| next_handle | `u64` | Monotonically increasing handle counter |
| logger | `Option<Arc<dyn ILogger + Send + Sync>>` | Logger reference |

### ControlMessage

Messages sent to the actor's main MPSC channel.

| Variant | Fields | Description |
|---------|--------|-------------|
| ConnectClient | `ClientSession` | Register a new client |
| DisconnectClient | `client_id: u64` | Unregister a client |
| Shutdown | (none) | Graceful shutdown signal |

### ClientSession

Per-client channel state held by the actor.

| Field | Type | Description |
|-------|------|-------------|
| id | `u64` | Unique client identifier |
| ingress_rx | `Receiver<Command>` | Receive commands from client |
| callback_tx | `Sender<Completion>` | Send completions to client |
| pending | `VecDeque<Completion>` | FIFO backlog of completions that couldn't be delivered because the client's callback channel was full; retried by `flush_pending` each poll cycle so a stalled client doesn't head-of-line-block delivery to others |

### InflightOp

Tracking state for an in-flight async io_uring operation.

| Field | Type | Description |
|-------|------|-------------|
| handle | `OpHandle` | Client-facing handle |
| client_id | `u64` | Which client submitted this |
| deadline | `Option<Instant>` | Timeout deadline (if timeout_ms > 0) |
| is_read | `bool` | Whether this is a read or write |
| tag | `u64` | io_uring user_data tag correlating CQEs back to this op |
| start_ns *(telemetry feature only)* | `u64` | Intended op-start timestamp for latency accounting — currently computed as a freshly-created `Instant`'s `elapsed()` (~0) and never read on completion; see align-tasks (telemetry-latency defect) |
| bytes *(telemetry feature only)* | `u64` | Byte count recorded for this op |

## State Transitions

### Component Lifecycle

```
Created → Configured → Initialized → Running → ShutDown
```

- **Created**: `define_component!` instantiates with None/default fields
- **Configured**: `set_file_path()`, `set_block_size()`, `set_num_blocks()` called
- **Initialized**: `initialize()` opens/creates file, starts actor thread
- **Running**: Clients can connect and perform IO
- **ShutDown**: `shutdown()` stops actor, closes file

### IO Operation States (Async)

```
Submitted → InFlight → Completed | TimedOut | Aborted
```

- **Submitted**: Command received on client ingress channel
- **InFlight**: SQE submitted to io_uring, tracked in `inflight` map
- **Completed**: CQE harvested, Completion sent to client
- **TimedOut**: Deadline expired, Completion::Timeout sent
- **Aborted**: AbortOp received, AsyncCancel submitted

## Relationships

```
BlockDeviceFilesysComponent 1──1 FilesysActor (via ActorHandle)
FilesysActor 1──* ClientSession (connected clients)
FilesysActor 1──* InflightOp (pending async operations)
ClientSession 1──1 Receiver<Command> (ingress)
ClientSession 1──1 Sender<Completion> (callback)
```

# dispatch-map

Thread-safe dispatch map component for the Certus storage system. Maps extent keys to their current location (in-memory staging buffer or on-disk block device offset) with readers-writer reference counting for concurrent access. Part of the Certus project.

## Architecture

### Component Wiring

```
DispatchMapComponentV0 --> [IDispatchMap provider]
                       <-- [ILogger receptacle]
                       <-- [IExtentManager receptacle]
```

**Lifecycle**: `new_default()` → bind receptacles → initialize (recovery from extent manager) → use `IDispatchMap` methods.

### Key Interfaces

| Interface | Role | Description |
|-----------|------|-------------|
| `IDispatchMap` | Provided | Extent key lookup, staging, storage commit, reference counting |
| `ILogger` | Receptacle | Info, debug, and error logging via dependency injection |
| `IExtentManager` | Receptacle | Extent iteration for recovery on initialization |

### Data Model

Each entry in the dispatch map is keyed by a `CacheKey` (`u64`) and stores:

| Field | Description |
|-------|-------------|
| Location | Either a DMA staging buffer or a block-device offset + device ID |
| Extent manager ID | Identifies which extent manager owns the extent |
| Size | Extent size in 4KiB blocks |
| Read ref count | Atomic counter for concurrent readers |
| Write ref count | Atomic counter for exclusive writer |
| TSC timestamp | CPU timestamp counter value, set on creation and updated on lookup |

Per-entry metadata is kept compact (target: ≤32 bytes beyond the key).

### Entry Lifecycle

Entries follow a one-way state progression:

```
create_staging(key, size)          convert_to_storage(key, offset, id)
        │                                     │
        ▼                                     ▼
   ┌──────────┐                        ┌─────────────────┐
   │ Staging  │ ────────────────────>  │ Block Device     │
   │ (DMA buf)│                        │ (offset + devid) │
   └──────────┘                        └─────────────────┘
        │                                     │
        └──────────── remove(key) ◄───────────┘
```

To re-stage an entry, the caller must first `remove()` then `create_staging()` again.

### Concurrency Model

The dispatch map enforces readers-writer lock semantics per entry:

- **Multiple concurrent readers** are allowed when no writer is active.
- **A single writer** blocks until all readers and other writers have finished.
- `take_read` and `take_write` accept a **timeout parameter** and return a timeout error if the condition is not met within the deadline.
- `downgrade_reference` atomically converts a write lock to a read lock with no unprotected window.
- `remove()` returns an error if any references are active; the caller must drain all refs first.

All methods are thread-safe and re-entrant.

### IDispatchMap Methods

| Method | Description |
|--------|-------------|
| `create_staging(key, size)` | Allocate a DMA staging buffer, record entry with write_ref=1 |
| `lookup(key, timeout)` | Return location (DmaBuffer, BlockDeviceLocation, NotExist, ErrorMismatchSize); increments read ref and updates TSC |
| `convert_to_storage(key, offset, block_device_id)` | Transition entry from staging to on-disk location |
| `take_read(key, timeout)` | Wait for write_ref=0, then increment read_ref |
| `take_write(key, timeout)` | Wait for read_ref=0 and write_ref=0, then increment write_ref |
| `release_read(key)` | Decrement read_ref (error if already 0) |
| `release_write(key)` | Decrement write_ref (error if already 0) |
| `downgrade_reference(key)` | Atomically convert write ref to read ref |
| `remove(key)` | Delete entry (error if references active) |
| `oldest_keys(n)` | Return up to `n` keys with the lowest TSC values (least recently created/accessed) |

### Recovery

On initialization, the component calls `IExtentManager::for_each_extent` to iterate all committed extents and populate the map. This ensures previously persisted data is immediately available for lookup after restart.

### Error Handling

All invalid operations return errors (no panics, no silent no-ops):

- Reference count underflow (`release_read`/`release_write` when count is 0)
- Downgrade without a write reference held
- `create_staging` with size=0 or DMA allocation failure
- `remove` with active references
- Timeout exceeded on blocking operations

## Prerequisites

- Linux host with hugepages configured and IOMMU enabled
- SPDK built at `deps/spdk-build/` (run `deps/build_spdk.sh`)
- Rust stable toolchain (edition 2021, MSRV 1.75+)

## Build

```bash
cargo build -p dispatch-map
```

## Tests

```bash
cargo test -p dispatch-map
```

## Lint and Format

```bash
cargo fmt -p dispatch-map --check
cargo clippy -p dispatch-map -- -D warnings
cargo doc -p dispatch-map --no-deps
```

## Source Layout

```
src/
  lib.rs          Component definition, IDispatchMap impl
info/
  FUNCTIONAL-DESIGN.md   Original functional design input
  PROMPT.md              Design prompt
specs/
  001-dispatch-map/
    spec.md              Feature specification
    checklists/          Quality validation checklists
```

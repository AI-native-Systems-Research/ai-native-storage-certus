# Data Model: Tests and Benchmarks

## Test Infrastructure Entities

### MockBlockDevice

In-memory block device implementing `IBlockDevice` for testing.

| Field | Type | Description |
|-------|------|-------------|
| blocks | HashMap<u64, [u8; 4096]> | LBA → block data mapping |
| sector_size | u32 | Always 4096 for tests |
| num_sectors | u64 | Configurable capacity |
| fault_config | Arc<Mutex<FaultConfig>> | Fault injection control |
| actor_thread | JoinHandle | Background command processor |

### FaultConfig

Controls fault injection behavior in the mock.

| Field | Type | Description |
|-------|------|-------------|
| fail_after_n_writes | Option<u32> | Fail after N successful writes (countdown) |
| fail_lba_range | Option<(u64, u64)> | Fail writes to LBAs in [start, end) |
| fail_all_writes | bool | Fail all subsequent writes |

### HeapDmaBuffer

Utility for creating `DmaBuffer` from heap memory (bypasses SPDK).

| Operation | Description |
|-----------|-------------|
| alloc(size, align) | Allocate zeroed heap memory, wrap via DmaBuffer::from_raw() |
| free_fn | C-ABI deallocator matching alloc layout |

## Relationships

```
ExtentManagerComponentV1
  └── block_device receptacle → MockBlockDevice (in tests)
  └── logger receptacle → (optional, can remain unwired in tests)
  └── flush_fn → no-op closure (mock needs no polling)

MockBlockDevice
  └── SpscChannel<Command> → actor thread → SpscChannel<Completion>
  └── blocks: in-memory storage
  └── fault_config: shared with test code for injection
```

## State Transitions (Mock Actor)

```
Idle → Receive Command
  → ReadSync:  lookup blocks[lba], write into DmaBuffer, send ReadDone
  → WriteSync: check fault_config, if ok: blocks[lba] = data, send WriteDone
                                   if fail: send Error or WriteDone with Err
  → Other:     send appropriate default Completion
```

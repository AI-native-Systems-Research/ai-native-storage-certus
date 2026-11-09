# Tasks: Extent Manager V2

**Spec**: [spec.md](spec.md) | **Plan**: [plan.md](plan.md)

## Completed

- [x] Define `IExtentManager` trait with two-phase reserve/publish API
- [x] Implement buddy allocator (data device), slab allocator, bitmap
- [x] Implement `RegionState` with per-slab key vectors and BTreeMap slab storage
- [x] Implement `SizeClassManager` using start_offsets as slab identifiers
- [x] Implement `write_checkpoint` / `read_checkpoint_region` (dual-copy, CRC32)
- [x] Implement `initialize()` recovery from checkpoint (key vectors → bitmaps)
- [x] Implement `get_extents()` / `for_each_extent()` (iterate slab key vectors)
- [x] Implement `remove_extent(offset: u64)` with BTreeMap range lookup
- [x] Implement deferred slot freeing (`pending_frees` flushed post-checkpoint)
- [x] Implement FREE_KEY (u64::MAX) silent-discard in `publish()`
- [x] Implement `region_for_offset()` using uniform partition arithmetic
- [x] Implement superblock (v6 / CERTUSV4, CRC32, dual-copy checkpoint metadata)
- [x] Remove `lookup_extent()` from `IExtentManager` interface
- [x] Remove `DuplicateKey` / `KeyNotFound` error variants; add `OffsetNotFound`
- [x] Default DMA allocation to SPDK `DmaBuffer::new()` in production;
      `set_dma_alloc()` exposed only on the concrete type for test injection
- [x] Write integration tests: lifecycle, checkpoint, concurrent, edge_cases
- [x] Write Criterion benchmarks: reserve_publish, enumerate, remove, checkpoint
- [x] Update spec.md to reflect key-vector design (no index, offset-based remove,
      FREE_KEY, BTreeMap, CERTUSV4)
- [x] Update plan.md and tasks.md to match current implementation

## Open

- [ ] Review on-disk format tables in spec.md against live superblock/checkpoint
      code — verify byte offsets, field sizes, and CERTUSV4 magic value
- [x] Runtime-configurable checkpoint interval — resolved: exposed on the interface
      as `set_checkpoint_interval(Option<Duration>)` (FR-027); the interval is held
      in `CheckpointTimerState` (Mutex<Option<Duration>> + Condvar), not an
      `AtomicU64`, and `None` disables the background timer.
- [ ] Consider whether incremental checkpointing (delta / WAL) is a tracked
      future requirement or permanently out of scope
- [ ] Mark spec status as "Draft" or "Approved" after final review
- [ ] Run `cargo bench -p extent-manager` end-to-end on hardware to validate
      the 100M-extent scale claim in SC-005

# Tasks

## Review Backfilled Spec
- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Remove implementation notes that don't belong in spec
- [ ] Add any missing requirements
- [ ] Mark spec status as "Draft" or "Approved"

## Add Pre-Flight Capacity Check to `put()`
- [ ] Track estimated serialized size of all entries (incremented on put, decremented on delete)
- [ ] Compare against region capacity before accepting a put operation
- [ ] Return `ExtendedMetadataStoreError::CapacityExhausted` when serialized size would exceed region capacity
- [ ] Add unit test: put entries until capacity is exhausted, verify error returned at put time (not only at flush time)
- [ ] Update spec FR section to document capacity-aware put behavior

## Enforce Explicit Key Length Limit
- [ ] Define `MAX_KEY_SIZE` constant (e.g., 4096 bytes) consistent with on-disk u16 key_len field
- [ ] Validate key length in `put()` and return a new `ExtendedMetadataStoreError::KeyTooLarge` variant
- [ ] Add the `KeyTooLarge` variant to the interface error enum in `components/interfaces/src/iextended_metadata_store.rs`
- [ ] Add unit tests for boundary key lengths (0, 1, MAX_KEY_SIZE, MAX_KEY_SIZE+1)

## Optimize Block I/O for Multi-Sector Batching
- [ ] Refactor `BlockDeviceClient::read_sectors` to issue a single multi-sector read command instead of one command per sector
- [ ] Refactor `BlockDeviceClient::write_sectors` to issue a single multi-sector write command instead of one command per sector
- [ ] Verify `MockBlockDevice` handles multi-sector commands correctly (may need `Command::ReadSync`/`WriteSync` with count field or new command variant)
- [ ] Benchmark flush latency before and after optimization on real NVMe hardware

## Add Operational Metrics and Telemetry
- [ ] Define a `MetadataStoreTelemetry` struct with: total puts, total gets, total deletes, flush count, flush latency histogram, entry count, dirty count, last flush timestamp
- [ ] Expose a `telemetry()` method on the component (or via a new `IMetadataStoreTelemetry` interface)
- [ ] Instrument `put`, `get`, `delete`, and `flush_to_disk` to update counters
- [ ] Add unit test verifying counters increment correctly

## Wire `force_flush` to FlushManager in Component
- [ ] Currently `force_flush()` in `IExtendedMetadataStore` impl is a no-op; it should delegate to the `FlushManager` when one is attached
- [ ] Add an optional `FlushManager` reference (or channel) to the component's fields
- [ ] Implement `force_flush()` to call `FlushManager::trigger_flush()` when the manager is present, returning errors properly
- [ ] Add integration test: `force_flush()` via the interface triggers actual persistence

## Replace Custom CRC32 with `crc32fast`
- [ ] Add `crc32fast` to `Cargo.toml` dependencies
- [ ] Replace the `crc32_of()` function in `on_disk.rs` with `crc32fast::hash()`
- [ ] Verify all existing on-disk format tests still pass (backward-compatible; same IEEE polynomial)
- [ ] Benchmark CRC computation on large values (128 KiB) to confirm SIMD speedup

## Add `drop`-Based Final Flush Guard to Component
- [ ] Currently `FlushManager::drop` performs a final flush, but the component has no ownership of the FlushManager
- [ ] Design lifecycle: component should hold an `Option<FlushManager>` that is constructed during `initialize_from_client`
- [ ] Implement `Drop` for the component (or a wrapper) that ensures the FlushManager is dropped (and thus final-flushed) on shutdown
- [ ] Add test: write entries, drop component, reboot, verify entries persisted

## Add Incremental Dirty Tracking for Future WAL Support
- [ ] Replace single `dirty_count: AtomicU64` with a `DirtyTracker` struct that records which keys changed since last flush
- [ ] On flush, only serialize changed entries (delta) rather than full snapshot (optimization for large stores)
- [ ] Maintain backward compatibility: full-region writes are still valid; delta optimization is an internal improvement
- [ ] Add benchmark comparing full-snapshot flush vs delta flush with 10,000 entries and 1% mutation rate

## Improve Documentation and Examples
- [ ] Add module-level doc comments to `block_io.rs`, `flush.rs`, `recovery.rs` explaining public API usage
- [ ] Add a doc example in `lib.rs` showing the full lifecycle: create component, initialize from client, put/get, force_flush
- [ ] Ensure `cargo doc --no-deps -p extended-metadata-store` produces no warnings
- [ ] Add `# Panics` and `# Errors` doc sections to all public methods

# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Remove implementation notes that don't belong in spec
- [ ] Add any missing requirements
- [ ] Mark spec status as "Draft" or "Approved"

## Documentation Gaps

- [ ] Add doc examples to `ISPDKEnv` trait methods (`init`, `fini`, `devices`, `device_count`, `is_initialized`)
- [ ] Add doc examples to `IExtendedMetadataStore` trait methods (`put`, `get`, `delete`, `iterate_all`, `force_flush`)
- [ ] Add doc examples to `IDispatchMap` trait methods (currently have only `// Verified:` annotations but no `# Examples` blocks)
- [ ] Add doc examples to `IMemoryTier` trait methods (missing `# Examples` blocks)
- [ ] Add module-level documentation to `iextended_metadata_store.rs` (currently has no `//!` module doc)
- [ ] Verify `cargo doc -p interfaces --no-deps` produces zero warnings

## Error Type Consistency

- [ ] Audit all error enums implement `std::error::Error` (currently verified: all 9 do)
- [ ] Consider adding `#[non_exhaustive]` to error enums to allow future variant additions without breaking changes
- [ ] Evaluate adopting `thiserror` derive macro to reduce Display impl boilerplate across 9 error enums (~200 lines)
- [ ] Add `source()` implementations where error enums wrap inner errors (e.g., `NvmeBlockError::BlockDevice`, `NvmeBlockError::SpdkEnv`)

## Testing Improvements

- [ ] Add unit tests for `ExtendedMetadataStoreError` display formatting (currently untested)
- [ ] Add unit tests for `PartitionTableError` display formatting (currently untested)
- [ ] Add unit tests for `ExtentManagerError` display formatting (currently untested)
- [ ] Add unit tests for `SpdkEnvError` display formatting (requires `spdk` feature)
- [ ] Add unit tests for `BlockDeviceError` display formatting (requires `spdk` feature)
- [ ] Add static assertion tests (`fn assert_send_sync<T: Send + Sync>()`) for `DmaBuffer`, `GpuStream`, `GpuIpcHandle`, `GpuDmaBuffer`, `IpcHandle`
- [ ] Add `WriteHandle` two-phase commit test: verify auto-abort on drop when neither `publish()` nor `abort()` is called
- [ ] Add `FormatParams::new()` constructor test verifying defaults
- [ ] Add `DispatcherConfig::default()` test verifying all default values match spec (FR-021)

## Feature Gate Hygiene

- [ ] Verify that `IpcHandle` (currently exported without `spdk` gate) is intentional -- it contains a raw pointer to GPU memory but is used by `IRemoteLookup` which is always available
- [ ] Verify that `DispatcherConfig` and `DispatcherError` (currently always available) are intentional for configuration-only consumers
- [ ] Verify that `FormatParams`, `WriteHandle`, `Extent`, `ExtentKey`, `ExtentManagerError` (currently always available) are intentional
- [ ] Document the rationale for exporting error/config types without feature gates (for error handling in non-hardware code paths)

## Safety Audit

- [ ] Review all `unsafe impl Send` declarations have adequate `// SAFETY:` justification
- [ ] Review all `unsafe impl Sync` declarations have adequate `// SAFETY:` justification
- [ ] Audit `DmaBuffer::from_raw` safety contract -- caller must guarantee `free_fn` matches allocator
- [ ] Audit `GpuDmaBuffer::new` safety contract -- caller must guarantee pointer validity
- [ ] Consider replacing `unsafe impl Send for Command` with a safe wrapper pattern (Command contains `Arc<Mutex<DmaBuffer>>` which is already `Send`)

## Code Quality

- [ ] Run `cargo clippy -p interfaces -- -D warnings` and resolve any diagnostics
- [ ] Evaluate splitting `igpu_services.rs` (737 LOC) -- types could move to a `gpu_types.rs` module paralleling `spdk_types.rs`
- [ ] Evaluate splitting `idispatcher.rs` (665 LOC) -- `DispatcherConfig` and tests could be separate submodules
- [ ] Add `#[must_use]` attribute to `WriteHandle` to prevent accidental drops without publish/abort
- [ ] Add `#[must_use]` to `LookupRef` to remind callers to call `release_lookup`
- [ ] Consider making `EvictionHandle` fields private with only accessor methods (currently has `pub` constructor but private fields -- this is correct)

## Formal Verification Alignment

- [ ] Cross-reference Creusot property comments against actual proof files in component `verif/` directories
- [ ] Verify that "unchecked" properties have corresponding GitHub issues or testing TODOs
- [ ] Update property counts if new proofs have been added since spec generation

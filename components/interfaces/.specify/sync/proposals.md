# Spec Sync Proposals — `interfaces`

**Generated**: 2026-07-21
**Spec**: `components/interfaces/specs/001-interfaces/spec.md`
**Base commit**: `833e9f36e01f1df8a0e0fc57d5cd223d823d3199`
**Next available FR numbers**: FR-027, FR-028 (highest existing is FR-026)

---

## Proposal 1 — DispatcherConfig cold-load staging fields

- **Direction**: BACKFILL
- **Current State**: FR-018 describes `DispatcherConfig` as a "14-field
  configuration". The struct now has 16 fields: `cold_staging_slots` (usize,
  default 64) and `cold_staging_buf_bytes` (usize, default 4 MiB) were added at
  `src/idispatcher.rs:81-87` (defaults `:109-110`) and are undocumented.
- **Proposed Resolution**:
  - Add new **FR-027: IDispatcher Cold-Load Staging Configuration**:

    > The dispatcher SHALL maintain a bounded pool of pre-registered pinned host DRAM staging buffers for SSD→GPU cold loads that cannot obtain a memory-tier slot under pressure, sized by `DispatcherConfig::cold_staging_slots` (buffer count, default 64; 0 disables staging so cold loads fail on a full memory tier) and `DispatcherConfig::cold_staging_buf_bytes` (per-buffer byte capacity, must be ≥ the largest per-block transfer size, default 4 MiB), bounding concurrent cold-read parallelism so a burst cannot exhaust the memory tier.

  - Update **FR-018** first bullet: change "14-field configuration" to "16-field
    configuration (… plus cold-staging slots and buffer size)".
- **Rationale**: Additive fields with defaults; backward compatible via `Default`.
  Backfilled spec predates the multi-GPU cold-staging work.
- **Confidence**: High
- [ ] Approved

---

## Proposal 2 — IGpuServices multi-GPU device routing methods

- **Direction**: BACKFILL
- **Current State**: FR-011 lists `IGpuServices` methods but omits the two new
  methods at `src/igpu_services.rs:532-578`: `set_device(&self, device: i32) ->
  Result<(), String>` and `device_of_ptr(&self, ptr: *const c_void) ->
  Result<i32, String>`.
- **Proposed Resolution**:
  - Add new **FR-028: IGpuServices Multi-GPU Device Routing**:

    > `IGpuServices` SHALL provide `set_device(device: i32) -> Result<(), String>` to bind the calling OS thread's current CUDA device (CUDA tracks the current device per thread; required before creating a stream or issuing a DMA for a specific GPU) and `device_of_ptr(ptr: *const c_void) -> Result<i32, String>` to return the CUDA device ordinal owning a device pointer via `cudaPointerGetAttributes` (`-1` for a pointer with no device association, e.g. host memory), so DMAs can be routed to a stream on the pointer's own device under multi-GPU / tensor parallelism.

  - Append two bullets to the **FR-011** method list:
    - `set_device(&self, device: i32) -> Result<(), String>` - Select the calling thread's current CUDA device.
    - `device_of_ptr(&self, ptr: *const c_void) -> Result<i32, String>` - Return the CUDA device ordinal owning a device pointer (-1 if unknown).
- **Rationale**: Additive trait methods for multi-GPU routing; no existing method
  changed.
- **Confidence**: High
- [ ] Approved

---

## Proposal 3 — Completion enum Clone derive (entity-attribute note)

- **Direction**: BACKFILL
- **Current State**: `Completion` changed from `#[derive(Debug)]` to
  `#[derive(Debug, Clone)]` at `src/iblock_device.rs:350`. FR-017 lists
  `Completion` as a "10-variant enum" and NFR-003 states only that "`Command` and
  `Completion` are `Send`". The `Clone` capability is undocumented.
- **Proposed Resolution** (amendment, no new FR needed):
  - Amend the **FR-017** `Completion` bullet to read:

    > `Completion`: 10-variant enum for operation results; derives `Clone` (in addition to `Debug`) so the block-device actor can `try_send` a clone of a completion on a full ring without consuming the original, enabling non-blocking completion delivery.

  - Amend **NFR-003** to note `Completion` is `Send + Clone` (`Command` remains `Send`).
- **Rationale**: Small additive derive; purely enables non-blocking delivery, does
  not alter thread-safety guarantees.
- **Confidence**: High
- [ ] Approved

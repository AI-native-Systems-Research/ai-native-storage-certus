# Spec Drift Report — `interfaces`

**Generated**: 2026-07-21
**Spec**: `components/interfaces/specs/001-interfaces/spec.md`
**Base commit**: `833e9f36e01f1df8a0e0fc57d5cd223d823d3199`
**Head**: `HEAD` (branch `unstable`)
**Scope**: `src/idispatcher.rs`, `src/igpu_services.rs`, `src/iblock_device.rs`

## Summary

| Metric | Count |
|--------|-------|
| Spec FRs total | 26 (FR-001..FR-026) |
| Aligned | 24 |
| Drifted (spec understates code) | 2 (FR-011, FR-018) |
| Entity-attribute drift | 1 (FR-017 / NFR-003 — `Completion` now `Clone`) |
| Not-Implemented (in spec, absent in code) | 0 |
| Conflicts | 0 |
| Unspecced code features | 3 |

All drift is **additive**: the implementation grew new fields/methods and one
derive that the backfilled spec predates. No spec requirement is contradicted or
removed. Recommended direction is **BACKFILL** (bring spec up to code).

## Per-Spec Classification

### `components/interfaces/specs/001-interfaces/spec.md`

| FR / NFR | Subject | Status | Note |
|----------|---------|--------|------|
| FR-008 | IDispatcher methods | Aligned | Method set unchanged by this diff. |
| FR-011 | IGpuServices methods | **Drifted** | Two new methods `set_device`, `device_of_ptr` not listed. |
| FR-017 | Supporting Types — Block Device | **Drifted (entity attr)** | `Completion` now derives `Clone`; spec/NFR-003 only note `Send`/`Debug`. |
| FR-018 | Supporting Types — Dispatcher | **Drifted** | `DispatcherConfig` now has 16 fields (spec says 14); two cold-staging fields missing. |
| FR-022 | Supporting Types — GPU Services | Aligned | No new GPU entity types introduced. |
| NFR-003 | Thread Safety | Aligned (note) | `Completion` remains `Send`; `Clone` is an added capability, not a change to safety guarantees. |
| all others | — | Aligned | Untouched by this diff. |

## Unspecced Code Table

| Feature | Location | Suggested FR |
|---------|----------|--------------|
| `DispatcherConfig::cold_staging_slots` (usize, default 64) + `cold_staging_buf_bytes` (usize, default 4 MiB) — bounded pinned-DRAM staging pool for SSD→GPU cold reads | `src/idispatcher.rs:81-87`, defaults at `:109-110` | New **FR-027** (+ update FR-018 field count to 16) |
| `IGpuServices::set_device(device: i32) -> Result<(), String>` and `device_of_ptr(ptr: *const c_void) -> Result<i32, String>` — per-thread CUDA device selection + device-ownership query for multi-GPU routing | `src/igpu_services.rs:532-578` | New **FR-028** (+ add both methods to FR-011 list) |
| `Completion` enum gains `Clone` (was `#[derive(Debug)]`, now `#[derive(Debug, Clone)]`) — lets the actor `try_send` a clone on a full ring without consuming the original | `src/iblock_device.rs:350` | Amend **FR-017** entity note (+ NFR-003 mention) |

## Conflicts

None. All changes are additive and backward-compatible.

## Recommendations

Direction for every item: **BACKFILL** (spec trails code; code is the intended
behavior). Exact proposed FR text is in `proposals.md`. Summary:

1. **Add FR-027 — IDispatcher Cold-Load Staging Configuration.** Document the
   bounded pinned-DRAM staging pool (`cold_staging_slots`,
   `cold_staging_buf_bytes`) and update FR-018 to say 16 fields.
2. **Add FR-028 — IGpuServices Multi-GPU Device Routing.** Document `set_device`
   and `device_of_ptr`; also append both to the FR-011 method list.
3. **Amend FR-017** (and NFR-003) to record that `Completion` derives `Clone` for
   non-blocking completion delivery.

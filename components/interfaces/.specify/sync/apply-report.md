# Spec Sync Apply Report — `interfaces`

**Applied**: 2026-07-21
**Spec**: `components/interfaces/specs/001-interfaces/spec.md`
**Base commit**: `833e9f36e01f1df8a0e0fc57d5cd223d823d3199`
**Backup**: `components/interfaces/.specify/sync/backups/spec.md.bak`
**Direction**: BACKFILL (all proposals approved)

## Changes Applied

| Proposal | FR | Action | Result |
|----------|----|--------|--------|
| P1 | FR-027 | Added | New FR "IDispatcher Cold-Load Staging Configuration (feature: spdk)" documenting `cold_staging_slots` / `cold_staging_buf_bytes`. |
| P1 | FR-018 | Amended | Field count 14 → 16; description now mentions cold-load staging slots and buffer size. |
| P2 | FR-028 | Added | New FR "IGpuServices Multi-GPU Device Routing" documenting `set_device` and `device_of_ptr`. |
| P2 | FR-011 | Amended | Appended `set_device` and `device_of_ptr` method bullets to the IGpuServices method list. |
| P3 | FR-017 | Amended | `Completion` bullet now records the `Clone` derive for non-blocking completion delivery. |
| P3 | NFR-003 | Amended | Thread-safety bullet now states `Completion` is `Send + Clone`. |

## Notes

- Only `.specify/sync/` artifacts and `spec.md` were modified. No source code changed.
- FR numbering remains contiguous (FR-001..FR-028).
- `proposals.json` updated with `"approved": true` for P1, P2, P3.

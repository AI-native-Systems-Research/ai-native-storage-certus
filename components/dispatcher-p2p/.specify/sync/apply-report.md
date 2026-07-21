# Spec Sync Apply Report
Applied: 2026-07-21
Project: dispatcher-p2p
Spec: 001-gpudirect-cold-path
Base commit: 833e9f36e01f1df8a0e0fc57d5cd223d823d3199

## Actions
1. Backed up spec.md → `.specify/sync/backups/spec.md.bak`.
2. Added **FR-015** to `specs/001-gpudirect-cold-path/spec.md` (after FR-014), documenting that the `IGpuServices` receptacle now exposes `set_device`/`device_of_ptr` for multi-GPU device selection. Wording explicitly scopes this as an interface keep-up: the capability is present in the receptacle/mock (only the test mock implements it), and the production cold path does NOT yet route transfers by device (per-device routing not wired into `pipelined_ssd_to_gpu_p2p`).
3. Marked Proposal 1 (`FR-015`) `"approved": true` in `proposals.json`.

## Result
| Proposal | Requirement | Direction | Status |
|----------|-------------|-----------|--------|
| 1 | FR-015 | BACKFILL (code authoritative) | Applied |

- spec.md functional requirements: FR-001..FR-014 → FR-001..FR-015.
- No existing FR modified; no conflicts.
- Backup available at `.specify/sync/backups/spec.md.bak` for rollback.

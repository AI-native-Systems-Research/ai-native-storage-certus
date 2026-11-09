# Align Tasks — extent-manager

Generated: 2026-08-07T16:02:56Z
Branch: `sync/spec-drift-sweep-20260807`
Source: proposals 2026-08-07T16:02:56Z (drift-report 2026-08-07T15:31:39Z)

These are spec→code (ALIGN) items. Per the user's decision for HIGH code/build
bugs, the fixes were **drafted on the feature branch** (not committed to
`unstable`) for review.

---

## Task 1: FR-030 — make `volatile_write_cache` compile and flush for real  [HIGH]

**Spec Requirement**: FR-030 (backfilled 2026-08-07 to "enabled = issue flush").
**Problem (before)**: Enabling `--features volatile_write_cache` failed to compile
in three places across two crates:
- `extent-manager/src/lib.rs:309-310` and `checkpoint.rs:102-103` call
  `metadata_client.flush()` — but `BlockDeviceClient` had no `flush()` method.
- `extent-manager/src/test_support.rs:231` (mock) referenced
  `Command::FlushSync` / `Completion::FlushDone`, which did not exist in the
  `interfaces` crate.

The feature was therefore entirely non-functional, and its spec wording was
additionally inverted ("compiled out when enabled").

**Change (drafted on branch)**:
- `components/interfaces/src/iblock_device.rs`: added `Command::FlushSync { ns_id }`
  and `Completion::FlushDone { handle, result }`.
- `components/extent-manager/src/block_io.rs`: added
  `BlockDeviceClient::flush()` (feature-gated on `volatile_write_cache`) that
  sends `FlushSync` and awaits `FlushDone`, mirroring `write_blocks`.
- `components/block-device-spdk-nvme/src/actor.rs`: added a `Command::FlushSync`
  dispatch arm and a `do_sync_flush()` helper issuing
  `spdk_nvme_ns_cmd_flush` (binding already allowlisted in `spdk-sys/build.rs`),
  modeled on `do_sync_write`.
- `components/extent-manager/specs/.../spec.md`: FR-030 wording corrected.

**Files Modified**:
- `components/interfaces/src/iblock_device.rs`
- `components/extent-manager/src/block_io.rs`
- `components/block-device-spdk-nvme/src/actor.rs`

**Estimated Effort**: medium (cross-crate).

### Acceptance Criteria
- [x] `cargo build -p interfaces` — clean.
- [x] `cargo build` (default members) — clean.
- [x] `cargo build -p block-device-spdk-nvme` — clean (SPDK prebuilt).
- [x] `cargo build -p extent-manager --features volatile_write_cache` — clean
      (previously a compile error).
- [x] `cargo test -p extent-manager --features volatile_write_cache` — 16 passed
      (checkpoint round-trips exercise the mock flush path).
- [ ] **REMAINING**: add a CI job that builds `--features volatile_write_cache`
      to prevent silent re-breakage (guideline: extend
      `.github/workflows/rust.yml`, but note SPDK crates need a runner with the
      SPDK build — likely a separate self-hosted job).
- [ ] **HARDWARE REVIEW**: `do_sync_flush` issues a real
      `spdk_nvme_ns_cmd_flush`; verify on hardware that the flush actually
      forces the volatile write cache and that the completion/status handling
      matches the write path under fault conditions.

---

## Task 2: FR-016 — correct stale "five minutes" doc strings  [LOW]

**Spec Requirement**: FR-016 (its own text instructs these be corrected to 30s).
**Change (applied on branch)**:
- `components/interfaces/src/iextent_manager.rs:244`: "five minutes" → "30 seconds".
- `components/extent-manager/README.md:13`: "default 5 minutes" → "default 30 seconds".

**Estimated Effort**: small.

### Acceptance Criteria
- [x] Both doc strings now state 30 seconds, matching `lib.rs:112`.
- [x] `cargo build -p interfaces` — clean.

---

## Informational (not tasked here)

- `plan.md` / `tasks.md` / `README.md` still reference a `block_device`
  receptacle, a `v2/` source path, `AtomicU64` interval, and CERTUSV5/v5.
  `spec.md` and code agree (CERTUSV4, FORMAT_VERSION 6). Refresh these planning
  docs in a separate docs pass — out of scope for this drift sync.

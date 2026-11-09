# Align Tasks

Generated: 2026-07-22
Source: drift-report.md (2026-07-22 run)

These items describe code that does not match its spec because the code is
defective, dead, or stale — not because the spec is wrong. Per spec-sync
policy, specs are NOT rewritten to match this behavior. Fix the code (or the
non-spec companion doc named below) instead.

---

## Task: Align 001-spdk-nvme-block-device/FR-013

**Severity**: High

**Spec Requirement**: FR-013 / SC-007 — "The actor service thread MUST be
pinned to a core in the same NUMA zone as the NVMe controller device,"
verified at instantiation.

**Current Code**: `probe_controller()` hardcodes the NUMA node to `0` for
every controller ("NUMA node is not available from minimal bindings; default
to 0"). `set_actor_cpu()` / `initialize()` derive `target_cpu` from this
always-0 value, so pinning is only correct by coincidence when the device
happens to sit on NUMA node 0. On a multi-socket host with the NVMe device on
a non-zero node, the actor is silently pinned to the wrong NUMA zone with no
error or warning. This also undermines `iops-benchmark`'s worker-pinning
logic (spec 002 FR-025), which trusts `numa_node()` as ground truth.

**Required Change**: `probe_controller()` must read the real NUMA node for
the controller's PCI device (e.g. via
`/sys/bus/pci/devices/<bdf>/numa_node`, or an equivalent SPDK/sysfs lookup)
instead of hardcoding `0`. If the value is genuinely unavailable, surface an
explicit error/warning rather than silently defaulting.

**Files to Modify**: `components/block-device-spdk-nvme/src/lib.rs:213-236,324-325`

---

## Task: Align 001-spdk-nvme-block-device/FR-010

**Severity**: Medium

**Spec Requirement**: FR-010 / SC-005 — "System MUST expose device
information (capacity, max queue depth, IO queue count, max transfer size,
block/sector size, NUMA id, NVMe version) ... Device information queries
return values consistent with the physical hardware properties of the bound
NVMe controller."

**Current Code**: `NvmeController::attach()` hardcodes `nvme_version =
1.0.0` and `max_transfer_size = 131072` (128KB) for every controller
("Default version and transfer size (not available from minimal bindings)").
Unlike `num_io_queues` / `max_queue_depth`, which are read from real SPDK
opts a few lines above, these two fields never reflect the actual device's
Identify Controller data.

**Required Change**: Derive `version` and `max_transfer_size` from the
controller's actual Identify Controller data (VER register / MDTS) rather
than hardcoding, mirroring how `num_io_queues`/`max_queue_depth` are already
read from real SPDK opts. If the current minimal SPDK bindings genuinely
cannot expose these fields, that limitation should be raised with the
spdk-sys/spdk-env maintainers rather than silently faked in this component.

**Files to Modify**: `components/block-device-spdk-nvme/src/controller.rs:150-159`

---

## Task: Align 001-spdk-nvme-block-device/FR-011

**Severity**: High

**Spec Requirement**: FR-011 / SC-008 — "When compiled with the `telemetry`
feature, system MUST collect and expose min/max/mean IO latencies, total
operation count, and mean throughput" and "All public interface methods have
unit tests."

**Current Code**: `TelemetryStats::record()` takes 3 args (`latency_ns,
bytes, is_read`), but the `#[cfg(feature = "telemetry")]` tests
`stats_record_single_op` / `stats_record_multiple_ops` call
`stats.record(1000, 4096)` with only 2 args. Confirmed by compilation:
`cargo test -p block-device-spdk-nvme --features telemetry --no-run`
produces 4x `error[E0061]: this method takes 3 arguments but 2 arguments
were supplied`. The telemetry feature cannot be tested at all in its current
state; the default (non-telemetry) build/tests compile fine.

**Required Change**: Update `stats_record_single_op` /
`stats_record_multiple_ops` to pass the required third `is_read: bool`
argument to `record()` so `cargo test --features telemetry` compiles and
passes.

**Files to Modify**: `components/block-device-spdk-nvme/src/telemetry.rs:206,218-220`
(signature at `src/telemetry.rs:60`)

---

## Task: Align 001-spdk-nvme-block-device/dead-code-DisconnectClient

**Severity**: Low

**Spec Requirement**: No spec requirement covers `ControlMessage`
variants directly; FR-019 specifies client-disconnect handling via the
`ChannelError::Closed` detection path only.

**Current Code**: `ControlMessage::DisconnectClient { client_id: u64 }` is
defined and matched in the actor's control-message loop but is never
constructed/sent anywhere in the codebase. The real disconnect path is
detecting `ChannelError::Closed` when polling a client's channel.

**Required Change**: Either remove the dead `DisconnectClient` variant and
its match arm, or wire it up if an explicit disconnect-by-control-message
path was intended as an alternative to closed-channel detection. Do not
leave it as unreachable dead code.

**Files to Modify**: `components/block-device-spdk-nvme/src/command.rs:65`,
`components/block-device-spdk-nvme/src/actor.rs:1216`

---

## Task: Align 001-spdk-nvme-block-device/unused-crossbeam-dependency

**Status**: ✅ RESOLVED 2026-08-07 (branch `sync/spec-drift-sweep-20260807`) — removed
`crossbeam-channel` from `Cargo.toml` and corrected the stale doc comments in
`benches/latency.rs:7` and `benches/throughput.rs:7` to describe the 256-slot
`component_core` `SpscChannel`. `cargo build -p block-device-spdk-nvme` and
`cargo bench -p block-device-spdk-nvme --no-run` both clean afterward.

**Severity**: Low

**Spec Requirement**: Spec Assumptions section (as of this sync) now
documents the production channel as `SpscChannel` bounded to
`CLIENT_CHANNEL_CAPACITY` (256); see backfilled note in
`specs/001-spdk-nvme-block-device/spec.md` Assumptions and
`research.md` R-002.

**Current Code**: `Cargo.toml` declares `crossbeam-channel = "0.5"` as a
dependency, but no production or test code constructs a crossbeam channel;
all client channels use `component_core::channel::spsc::SpscChannel`.
`benches/latency.rs` and `benches/throughput.rs` doc comments still claim
"crossbeam bounded channels (64 slots) as the SPSC transport," which is
stale/inaccurate.

**Required Change**: Remove the unused `crossbeam-channel` dependency from
`Cargo.toml` (or wire it in if there is a real use case), and correct the
doc comments in `benches/latency.rs` / `benches/throughput.rs` to describe
the actual `SpscChannel` (256-slot) transport instead of crossbeam.

**Files to Modify**: `components/block-device-spdk-nvme/Cargo.toml:21`,
`components/block-device-spdk-nvme/benches/latency.rs:7`,
`components/block-device-spdk-nvme/benches/throughput.rs:7`

---

## Task: Align 001-spdk-nvme-block-device/readme-channel-capacity

**Severity**: Low

**Spec Requirement**: N/A directly (companion documentation, not a spec
Markdown file — out of scope for this sync-apply pass, which may only edit
`specs/**` and `.specify/sync/**`). Flagged here for a human/code owner to
fix.

**Current Code**: `README.md:40` states "Channel capacity is 64 slots,"
but the actual constant is `CLIENT_CHANNEL_CAPACITY = 256` in `src/lib.rs`.

**Required Change**: Update `README.md` to state the correct channel
capacity (256 slots), consistent with the corrected Assumptions section in
`specs/001-spdk-nvme-block-device/spec.md`.

**Files to Modify**: `components/block-device-spdk-nvme/README.md:40`

---

## Summary Table

| Spec-ID/ID | Severity | Classification |
|---|---|---|
| 001/FR-013 | High | DEFECT (NUMA node hardcoded to 0) |
| 001/FR-010 | Medium | DEFECT (nvme_version/max_transfer_size hardcoded) |
| 001/FR-011 | High | DEFECT (telemetry test suite fails to compile) |
| 001/dead-code-DisconnectClient | Low | DEFECT (dead code) |
| 001/unused-crossbeam-dependency | Low | ALIGN (unused dep + stale bench doc comments) — ✅ RESOLVED 2026-08-07 |
| 001/readme-channel-capacity | Low | ALIGN (non-spec doc, out of edit scope) |

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`)

Drift source: `.specify/sync/drift-report.{json,md}` (generated 2026-08-07).
Pacing this sweep: auto-apply safe backfills/doc-softens; the one HIGH item
(FR-005 abort use-after-free) had its code fix **drafted on the branch** per the
user decision "Draft fix + queue task", staged for review — NOT committed to
`unstable`. Pre-edit backups in `.specify/sync/backups/20260807T160256Z/`.

## Task BD-1 (DRAFTED — needs hardware validation) — FR-005 abort use-after-free

**Severity**: High (memory safety)

**Spec Requirement**: FR-005 — abort an in-flight async op identified by its
component-assigned handle.

**Defect**: The old `AbortOp` handler (`actor.rs`) removed the `PendingOp` from
`pending_ops` and immediately delivered `Completion::AbortAck`. Removing the
`PendingOp` dropped its pinned `DmaBuffer` `Arc` while the NVMe command could
still be in flight — the controller could DMA into freed/reclaimed memory
(use-after-free). It also never issued a real NVMe abort, so the hardware
completion arrived later against an unknown handle and was silently discarded.

**Drafted fix (staged on branch, compiles clean, unit tests pass)**:
- `spdk-sys/build.rs`: allowlisted `spdk_nvme_ctrlr_cmd_abort_ext` (binding
  confirmed generated in `OUT_DIR/bindings.rs`).
- `PendingOp` gained `cmd_cb_arg: *mut c_void` (the leaked `AsyncIoContext`
  pointer, captured after `Box::into_raw` at both ReadAsync/WriteAsync submit
  sites) and `aborting: bool`.
- `AbortOp` handler: on a known outstanding handle, set `aborting = true`, issue
  `spdk_nvme_ctrlr_cmd_abort_ext(ctrlr, qpair, cmd_cb_arg, abort_completion_cb,
  null)`, and **keep** the `PendingOp` (and its buffer) alive. Unknown/already-
  completed handle → ack immediately (idempotent).
- `process_completions`: when the real completion for an `aborting` op arrives,
  deliver `Completion::AbortAck` (instead of Read/WriteDone) and only then drop
  the `PendingOp`, releasing the pinned buffer. Telemetry is not recorded for an
  aborted op.
- Added no-op `abort_completion_cb` (the aborted I/O's own callback is where the
  buffer is reclaimed; the abort admin completion carries nothing we need).

**Behavior change to validate**: `AbortAck` is now **deferred** until the
original command actually completes (aborted-by-request or otherwise), rather
than acked synchronously. This is the correct buffer-reclaim contract but
changes client-observable timing. **Cannot be exercised against real hardware in
this environment** — must be validated on an RDMA/NVMe test node before merge.

**Files**: `components/spdk-sys/build.rs`,
`components/block-device-spdk-nvme/src/actor.rs`.

### Related follow-up (NOT drafted) — `check_timeouts` has the same UAF shape

`check_timeouts()` (`actor.rs`, ~`fn check_timeouts`) removes a `PendingOp` from
`pending_ops` the moment its TSC deadline passes and delivers
`Completion::Timeout`, **without** issuing an NVMe abort — the same class of bug
as the old `AbortOp`: the buffer's `Arc` can drop while the command is still in
flight. The `handle_controller_reset` path resets the controller (which does
quiesce outstanding commands) but the plain per-op timeout path does not. This
should get the same treatment as BD-1 (issue a real abort + defer buffer release
until the hardware completion), tracked here as a related follow-up. Left
undrafted to keep BD-1 reviewable in isolation.

## Task BD-2 (OPEN — continues July FR-013 + FR-010) — real device NUMA / identify data

**Severity**: High (FR-013/SC-007) + Medium (FR-010/SC-005)

Supersedes the two July tasks "Align .../FR-013" and "Align .../FR-010" above as
a single hardware-discovery work item. The 2026-08-07 spec backfill **documents**
the current reality (NUMA node hardcoded 0; `nvme_version`=1.0.0 and
`max_transfer_size`=131072 fixed) as a known limitation in FR-010/FR-013/SC-005/
SC-007, so the spec is now honest — but the enhancement to read real values from
sysfs / SPDK Identify Controller (VER, MDTS) and the device's true NUMA node
remains open. Resolving it makes SC-005/SC-007 and iops-benchmark FR-025 pinning
fully correct.

**Files**: `components/block-device-spdk-nvme/src/lib.rs` (probe_controller NUMA),
`components/block-device-spdk-nvme/src/controller.rs:150-159` (version/MDTS).

## Task BD-3 (OPEN, Low) — iops-benchmark telemetry cross-check (spec 002 SC-006)

The iops-benchmark depends on `block-device-spdk-nvme` without the `telemetry`
feature, so SC-006's "cross-check with component telemetry" clause is inert
(backfilled in spec 002 to reflect this). To fully satisfy SC-006, enable the
`telemetry` feature on the dependency and add an optional post-run cross-check of
client-side latency stats against `telemetry()` output.

**Files**: `apps/iops-benchmark/Cargo.toml`, `apps/iops-benchmark/src/report.rs`.

## Resolved by BACKFILL this sweep (no code change; spec now documents reality)

- **FR-004** — async submit is fire-and-forget; `tag` is the client correlation
  key, handle not returned synchronously.
- **FR-028** — shipped `on_stop` order is drain → `Error{Aborted}` → park (safe
  because submission has already stopped); spec corrected from park-before-drain.
- **FR-030 (new)** — `read_write_stats()` / `ReadWriteStats` per-direction
  counters documented.
- **002/FR-022** — sync QD1 realized by actor-side serialization, not a strict
  worker submit-one-wait loop.
- **002/FR-024** — per-sub-op latency timing (not aggregate-per-batch).

---

# 2026-08-20 Sweep (Spec-Sync Phase B)

Drift source: `.specify/sync/drift-report.{json,md}` (generated 2026-08-20):
3 drifted requirements (all spec-lag → BACKFILL) + 8 unspecced features (all
BACKFILL-UNSPECCED). Classification per `.specify/sync/PHASE_B_POLICY.md`. No
`.rs` source was modified in this pass. One genuine (cosmetic) code defect was
uncovered inside unspecced feature #3 and is filed as an ALIGN task below.

## Task BD-4 (OPEN, Low) — per-device summary format string defect (spec 002 FR-026)

**Severity**: Low (cosmetic output defect)

**Spec Requirement**: FR-026 (backfilled this sweep) — when more than one device
is benchmarked, the final output includes a `=== Per-Device Summary ===` block
printing per-device IOPS and throughput.

**Current Code**: The per-device summary `println!` at
`apps/iops-benchmark/src/main.rs:423` has an unbalanced parenthesis in its
format string: `"\nDevice {} ({}: {:.0} IOPS, {:.1} MB/s"` — the `(` opened
after the PCI address is never closed, so each per-device line renders as e.g.
`Device 0 (0000:03:00.0: 120000 IOPS, 469.0 MB/s` with a dangling `(`. Purely
cosmetic; the numbers are correct.

**Required Change**: Fix the format string so the parenthesis is balanced, e.g.
`"\nDevice {} ({}): {:.0} IOPS, {:.1} MB/s"`. No behavioral change.

**Files to Modify**: `apps/iops-benchmark/src/main.rs:423`

### Acceptance Criteria

- [ ] The per-device summary line renders with balanced parentheses (PCI address
      wrapped in `( )`).
- [ ] No change to the reported IOPS / throughput values.
- [ ] `cargo build -p iops-benchmark` remains clean.

## Status update to prior tasks

- **Task BD-1 (FR-005 abort use-after-free)** — ✅ RESOLVED. The drafted
  defer-until-completion abort contract is now fully present in mainline
  (`src/actor.rs:972-1020` abort dispatch keeping the `PendingOp`+buffer and
  issuing `spdk_nvme_ctrlr_cmd_abort_ext`; `src/actor.rs:528-537` deferring
  `AbortAck` to the real completion). FR-005 spec text re-synced from "drafted on
  branch / needs hardware validation" to "implemented." (The related undrafted
  `check_timeouts` UAF-shape follow-up noted under BD-1 is not part of the
  current drift report and remains a separate concern.)
- **Task BD-2 (real device NUMA / identify data)** — STILL OPEN, but now narrower:
  `max_transfer_size` is in fact MDTS-derived in the shipped code
  (`src/controller.rs:169-177`), so FR-010/SC-005 were re-synced to drop it from
  the "fixed constants" list. BD-2 now covers only `nvme_version` (1.0.0) and the
  hardcoded NUMA node 0 (`src/lib.rs:333-334`).
- **Task BD-3 (iops-benchmark telemetry cross-check, SC-006)** — unchanged, STILL OPEN.

## Summary Table (2026-08-20)

| Spec-ID/ID | Severity | Classification |
|---|---|---|
| 002/FR-026 (BD-4) | Low | ALIGN (cosmetic format-string defect, `main.rs:423`) |
| 001/FR-005 (BD-1) | — | RESOLVED (abort contract implemented in mainline) |

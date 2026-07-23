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
| 001/unused-crossbeam-dependency | Low | ALIGN (unused dep + stale bench doc comments) |
| 001/readme-channel-capacity | Low | ALIGN (non-spec doc, out of edit scope) |

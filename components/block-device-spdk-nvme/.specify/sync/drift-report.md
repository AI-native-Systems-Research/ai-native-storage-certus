Generated: 2026-08-07T15:38:00Z

# Drift Report: block-device-spdk-nvme + iops-benchmark

Spec-vs-implementation drift analysis covering two specs:

1. **001-spdk-nvme-block-device** — the SPDK NVMe block device component
   (`components/block-device-spdk-nvme/src/*.rs`, interface in
   `components/interfaces/src/iblock_device.rs`). FR-001..FR-029, SC-001..SC-008.
2. **002-iops-benchmark** — the IOPS benchmark application
   (`apps/iops-benchmark/src/*.rs`). FR-001..FR-026 (incl. FR-006a/FR-006b),
   SC-001..SC-007.

Note: the component builds only with the `spdk` feature (SPDK crates are
excluded from workspace default-members); telemetry paths require the
`telemetry` feature.

## Summary

| Specs Analyzed | Requirements Checked | Aligned | Drifted | Not Implemented | Unspecced |
|----------------|----------------------|---------|---------|-----------------|-----------|
| 2              | 72                   | 62      | 10      | 0               | 7         |

Per-spec:

| Spec | Aligned | Drifted | Not Implemented |
|------|---------|---------|-----------------|
| 001-spdk-nvme-block-device | 30 | 7 | 0 |
| 002-iops-benchmark         | 32 | 3 | 0 |

Headline finding: the known sibling-component defect pattern (telemetry
`record` called with a hardcoded `0` latency) is **NOT present** here. Async
completions record a real TSC-derived latency (`actor.rs:161`,
`tsc.ticks_to_ns(rdtsc() - io_ctx.start)`) and the sync path records a real
delta (`actor.rs:613`). FR-011 / SC-006 latency accuracy is intact. The
material drift is instead that several hardware-derived device-info fields are
stubbed constants.

## Detailed Findings

### Aligned

**Spec 001**

- FR-001 IBlockDevice / `connect_client()` — `lib.rs:389`
- FR-002 Two SPSC channels per client (ingress `Command` + callback `Completion`) — `lib.rs:405-413`
- FR-003 Sync read/write (ns_id, DmaBuffer, LBA, no timeout) — `actor.rs:596,622`; `iblock_device.rs:243,252`
- FR-006 Write-zeros — `actor.rs:872`, `do_write_zeros` `actor.rs:1106`
- FR-007 Batch submit — `actor.rs:887`
- FR-008 Namespace probe/create/format/delete — `actor.rs:913-971`; `namespace.rs`
- FR-009 Controller reset with in-flight cancellation — `actor.rs:538`
- FR-011 Telemetry min/max/mean/total/throughput; error without feature — `telemetry.rs:60-167`
- FR-012 Single controller per instance, set_pci_address + initialize — `lib.rs:110-252`
- FR-014 Actor polls all client channels — `actor.rs:370`
- FR-015 Qpair pool [4,16,64,256], `io_queue_requests = depth*4`, shallowest-with-capacity, fallback most-available — `qpair.rs:141,173,258-279` (exact match)
- FR-016 ILogger receptacle — `lib.rs:78-81`, used throughout `actor.rs`
- FR-017 spdk-env for SPDK init — `lib.rs:130`
- FR-018 Client DmaBuffer + Arc refs — `iblock_device.rs:243-285`
- FR-019 Disconnect cancels in-flight + discards completions — `actor.rs:440-464,480-513`
- FR-020 Namespace ops serialized on actor thread — `actor.rs:913-971`
- FR-021 IBlockDeviceAdmin set_pci_address/set_actor_cpu/initialize/shutdown — `iblock_device.rs:503-539`; `lib.rs:342-376`
- FR-022 TscClock calibrated once, ~1ms timeout throttle — `tsc.rs:43-49`; `actor.rs:1234`
- FR-023 ContextPool slab allocator (acquire at submit, release in callback, no full pre-alloc) — `actor.rs:80-115,182`
- FR-024 Pre-allocated `completion_scratch` / `timeout_scratch` — `actor.rs:233,237,479,522`
- FR-025 ENOMEM retry to `min(timeout,1000ms)`, polls all qpairs, fails on deadline — `actor.rs:699-736,813-848`; `SUBMIT_ENOMEM_MAX_BACKPRESSURE_MS=1000` `actor.rs:35`
- FR-026 Non-blocking delivery + per-client FIFO backlog; `Completion: Clone` — `command.rs:35-56`; `iblock_device.rs:350`
- FR-027 `signal_stop()` (close channel, no join) + `detach_controller()` — `lib.rs:355-380`
- FR-029 Round-robin polling via rotating start index; `MAX_COMMANDS_PER_CLIENT_PER_POLL = 64` — `actor.rs:378,393-400`
- SC-001 Sync round-trip path present (perf unverifiable statically) — `actor.rs:1004-1103`
- SC-002 Async timeout via TSC deadline, ~1ms throttle — `actor.rs:519-535,1234`
- SC-003 Batch path forces a single shared qpair for the batch — `actor.rs:887-906`
- SC-004 Namespace lifecycle handled without leaks — `actor.rs:913-971`
- SC-006 Telemetry latency real (TSC-based), not hardcoded — `actor.rs:161,613`
- SC-008 Unit + doc tests present; perf covered by iops-benchmark + `benches/{latency,throughput}.rs`

**Spec 002**

- FR-001 `--op` read/write/rw default read — `config.rs:68`
- FR-002 `--block-size` default 4096 — `config.rs:74`
- FR-003 `--queue-depth` default 32 — `config.rs:85`
- FR-004 `--threads` default 1 — `config.rs:88`
- FR-005 `--duration` default 10 — `config.rs:92`
- FR-006 `--ns-id` default 1 — `config.rs:96`
- FR-006a `--pci-addr`, first device if omitted — `config.rs:100`; `main.rs:65-91`
- FR-006b `--pattern` random/sequential default random — `config.rs:108`
- FR-007 Startup validation (block-size multiple, threads/duration/qd >= 1) — `config.rs:124-174`
- FR-008 Clamp queue depth with warning — `config.rs:182-190`
- FR-009 Each thread connects via IBlockDevice — `main.rs:278`
- FR-010 Async pipeline kept full to queue depth — `worker.rs:103-139`
- FR-011 rw = 50/50 random — `worker.rs:221-227`
- FR-012 Config summary at startup (incl. io-mode) — `report.rs:9-31`
- FR-013 Per-second progress to stderr unless quiet — `main.rs:342-376`; `report.rs:37-62`
- FR-014 Stop-flag signal + join/collect — `main.rs:331-390`
- FR-015 Final summary total IOPS / MB/s / lat min/mean/p50/p99/max — `report.rs:65-133`; `stats.rs:56-122`
- FR-016 rw reports read/write IOPS separately + combined — `report.rs:105-120`
- FR-017 Random uniform LBA / sequential non-overlapping regions — `lba.rs:38-90`
- FR-018 Errors counted, not fatal — `worker.rs:249-276`
- FR-019 Exit 0 success / non-zero on error — `main.rs` (exit 1 validation, exit 2 fatal)
- FR-020 `--quiet` flag — `config.rs:116`
- FR-021 `--help` (clap) — derived
- FR-023 Comma-separated `--block-size` list, per-IO random pick — `config.rs:74`; `worker.rs:177-182`
- FR-025 Worker NUMA pinning via device info + OS affinity — `main.rs:222-253,296-302`
- FR-026 `--device-count` parallel multi-device, threads*device_count, aggregate, pci-addr precedence, validate >=1 — `main.rs:65-149,255-257,397-428`; `config.rs:162`
- SC-001 Barrier-gated start + timer within duration — `main.rs:262,328-339`
- SC-002 One connect_client per worker — `main.rs:278`
- SC-003 4KB random-read path present (perf unverifiable statically) — `worker.rs`
- SC-004 Validation before any IO — `main.rs:203-212`
- SC-005 Progress uses live op-counter deltas — `main.rs:354-374`
- SC-007 IO errors counted without crash/hang — `worker.rs:249-276`

### Drifted

**Spec 001**

- FR-004 (minor) — Async submit is fire-and-forget over the ingress channel; the
  component-assigned `OpHandle` is only returned later inside the completion, not
  synchronously at submission. Clients correlate via the caller-supplied `tag`,
  not the handle. `actor.rs:652-653,671`; `iblock_device.rs:261-285`.
- FR-005 (minor) — `AbortOp` only removes the op from `pending_ops` and acks; it
  does not cancel the outstanding SPDK command (the hardware completion is later
  silently discarded). Combined with FR-004, the client never learns the handle
  to abort a specific op. `actor.rs:907-912`.
- FR-010 (minor) — Device info partially stubbed: `nvme_version` hardcoded to
  1.0.0 (`controller.rs:153`), `max_transfer_size` hardcoded 131072
  (`controller.rs:158`), NUMA node forced to 0 at probe (`lib.rs:325`). Capacity,
  sector size, queue depth, io-queue count are real.
- FR-013 (minor) — Actor is pinned to a NUMA-local core, but the controller's
  NUMA node is hardcoded to 0 in `probe_controller` (`lib.rs:325`), so pinning
  always targets node 0 rather than the device's true node.
- FR-028 (minor) — Ordering: `on_stop` runs the 5s drain loop first
  (`actor.rs:1256-1273`), then delivers `Error{Aborted}` (`:1276-1284`), then
  parks the controller into the shared slot (`:1289-1291`). Spec text requires
  the controller be parked *before* the drain deadline is evaluated. Behaviorally
  the actor loop has already stopped submitting, so no new IO races, but the
  literal park-then-drain ordering in the spec is inverted.
- SC-005 (moderate) — Device info queries do not all reflect physical hardware:
  version/max-transfer-size/NUMA are constants (see FR-010), so a monitoring
  client gets fixed values regardless of the bound controller. `controller.rs:153-158`; `lib.rs:325`.
- SC-007 (moderate) — "Actor runs on a core in the same NUMA zone as the
  controller, verified at instantiation" is not truly satisfied: the controller
  NUMA node is never discovered (hardcoded 0), so the guarantee degenerates to
  "node 0." `lib.rs:220-236,325`.

**Spec 002**

- FR-022 (minor) — `--io-mode sync` does not enforce effective queue depth 1 at
  the worker: `run()` still fills the pipeline to `--queue-depth` and enqueues
  that many `ReadSync`/`WriteSync` commands (`worker.rs:112-116,128-131`). The
  actor serializes sync ops (blocking the single actor thread), so hardware QD is
  effectively 1, but the worker-side pipelining means measured sync latency
  includes channel/queue wait time rather than a strict submit-one-wait loop.
- FR-024 (minor) — `--batch-size N` groups N commands into `BatchSubmit`, but
  latency is measured per sub-operation (each op gets its own `Instant` pushed to
  `in_flight` and each completion is timed individually), not as the "aggregate
  latency of each batch" the spec requires. `worker.rs:159-172,189-190,237-268`.
- SC-006 (minor) — Final latency stats are computed correctly from actual
  client-side completion times (`worker.rs:241`; `stats.rs:93-104`), but the spec's
  verification "by cross-checking with telemetry output from the component" is not
  wired: the iops-benchmark crate depends on `block-device-spdk-nvme` without the
  `telemetry` feature (`apps/iops-benchmark/Cargo.toml`), so `telemetry()` would
  return an error and no cross-check is performed.

### Not Implemented

None. Every FR and SC in both specs has corresponding implementation.

## Unspecced Code

| Item | Location | Note |
|------|----------|------|
| `IBlockDevice::read_write_stats()` + `ReadWriteStats` type | `iblock_device.rs:494`, `lib.rs:511`, `telemetry.rs:140` | Per-direction read/write byte/op/latency counters; spec 001 only mentions aggregate min/max/mean/total/throughput telemetry. |
| `crossbeam-channel` dependency | `Cargo.toml` `[dependencies]` | Declared but unused in `src/` (production path uses `component_core` SpscChannel). Spec assumption already flags for cleanup via `align-tasks.md`. |
| `benches/completion_routing.rs` | `benches/` | Extra Criterion bench not referenced by SC-008 (which cites latency/throughput + iops-benchmark). |
| `--batch-size <= --queue-depth` validation | `config.rs:156-161` | Constraint not stated in spec-002 FR-024. |
| Per-device summary output block | `main.rs:397-428` | Beyond FR-026's "aggregate across all devices"; adds a per-device breakdown. |
| `[timing]` diagnostic lines (SPDK init, device init) | `main.rs:55,150-153` | Diagnostic stderr output not described in any FR. |
| Per-thread IOPS breakdown (progress + final report) | `report.rs:43-62,74-103` | Extra multi-thread detail beyond FR-013/FR-015. |

## Conflicts / Nonexistent References

None. Spec-001 references `align-tasks` (exists: `.specify/sync/align-tasks.md`)
and `apps/iops-benchmark` (exists). Spec-002 `apps/iops-benchmark/` and
`examples/` alternatives resolve to the existing `apps/iops-benchmark`.

Cosmetic note (not counted): `OpType::ReadWrite` `Display` prints `readwrite`
(`config.rs:22`) while the CLI value is `rw`, so the config summary shows
`Operation: readwrite` rather than `rw`.

## Recommendations

1. **Discover real device NUMA node** in `probe_controller` (`lib.rs:325`) instead
   of returning 0. This is the root cause of the two moderate drifts (SC-005,
   SC-007) and FR-013; it also makes iops-benchmark FR-025 pinning correct.
2. **Populate `nvme_version` and `max_transfer_size` from the controller** (via
   the real SPDK identify/opts APIs) rather than the 1.0.0 / 128KB constants in
   `controller.rs:153-158`, to satisfy FR-010 / SC-005.
3. **Surface the OpHandle to the client at submission** (or document that `tag` is
   the correlation mechanism and abort-by-handle is not client-usable) to resolve
   FR-004 / FR-005 ambiguity. Consider making `AbortOp` actually issue an NVMe
   abort rather than only discarding the completion.
4. **Reconcile FR-028 park/drain ordering** — either park (quiesce) the controller
   before the drain-deadline loop in `on_stop`, or update the spec text to match
   the implemented drain-then-park sequence.
5. **iops-benchmark FR-024**: measure and report aggregate per-batch latency when
   `--batch-size > 1`, or amend the spec to state per-op latency.
6. **iops-benchmark FR-022**: either strictly serialize submit/wait in sync mode
   (true QD1) or clarify that sync QD1 is realized via actor-side serialization.
7. **iops-benchmark SC-006**: enable the `telemetry` feature on the
   `block-device-spdk-nvme` dependency and add an optional telemetry cross-check,
   or relax the success-criterion wording.
8. **Remove the unused `crossbeam-channel` dependency** per the existing
   `align-tasks.md` cleanup note, and either document or drop `read_write_stats`
   and `benches/completion_routing.rs` in spec 001.

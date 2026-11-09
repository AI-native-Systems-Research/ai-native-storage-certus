# Tasks: GPU P2P DMA Server (`gpu-p2p-server` binary)

**Input**: Backfilled spec at `specs/003-gpu-p2p-server/spec.md`
**Status**: Draft (backfilled — needs human review). This tasks list
reflects work already completed in code prior to backfill; it is provided
for traceability, not as a forward implementation plan.

**Organization**: Tasks are grouped by user story to match `spec.md`.

## Phase 1: Setup

- [x] T001 Add `p2p` feature-gated `[[bin]]` target `gpu-p2p-server` in
  `components/gpu-services/Cargo.toml`
- [x] T002 Implement CLI argument parsing (`clap`) in
  `components/gpu-services/src/bin/p2p_server.rs`: `--socket`, `--pci`,
  `--mode`, `--staging-size`, `--chunk-size`, `--once`

## Phase 2: Foundational

- [x] T003 Implement `initialize_stack()` in
  `components/gpu-services/src/bin/p2p_server.rs`: kernel module checks
  (`nvidia_peermem`, `gdrdrv`), SPDK env init, GPU component init, NVMe
  block device open, `atexit` teardown-crash workaround
- [x] T004 Implement `parse_client_payload()` and `open_ipc_handle()` for
  decoding the base64 IPC handle line from a client connection

## Phase 3: User Story 1 - Benchmark NVMe-to-GPU Transfer Modes (P1)

- [x] T005 [US1] Implement `handle_bounce()` transfer path (NVMe → host DMA
  buffer → `cudaMemcpy` H2D → client GPU buffer)
- [x] T006 [US1] Implement `create_chunk_pool()` / `GpuStagingBuffer` and
  `handle_p2p()` transfer path (NVMe → pre-pinned GDRCopy staging → D2D →
  client GPU buffer)
- [x] T007 [US1] Implement `handle_p2p_cold()` transfer path (NVMe →
  per-request GDRCopy pin/map → D2D → client GPU buffer → unpin/unmap)
- [ ] T008 [US1] [NEEDS REVIEW] Add automated integration test exercising
  all three `--mode` values against a real or mocked NVMe device (currently
  validated manually / via `tests/gpu_nvme_p2p.rs` for the P2P DMA-buffer
  constructors only, not the server binary end-to-end)

## Phase 4: User Story 2 - Handle One Client Then Exit (P2)

- [x] T009 [US2] Implement `--once` flag: break accept loop and clean up
  after first client response

## Phase 5: User Story 3 - Graceful Shutdown and Cleanup (P3)

- [x] T010 [US3] Implement `SIGINT`/`SIGTERM` handler setting `SHUTDOWN`
  atomic flag, checked each accept-loop iteration
- [x] T011 [US3] Implement socket file removal and chunk pool drop on exit
  (both normal and `--once` paths)

## Phase 6: Polish (backfill follow-ups, not yet done)

- [ ] T012 [NEEDS REVIEW] Human review of this backfilled spec against
  original design intent (CLI defaults, wire-protocol stability guarantees)
- [ ] T013 [NEEDS REVIEW] Decide whether `gpu-p2p-server`'s wire protocol
  should be formally documented as a `contracts/` doc (currently only
  described narratively in `spec.md`)

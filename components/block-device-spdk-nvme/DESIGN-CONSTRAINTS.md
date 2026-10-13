# block-device-spdk-nvme: design constraints

This file gives the rules of `block-device-spdk-nvme` that other components depend on.
If a change breaks one of these rules, memory can become corrupt, a reader can hang, or a power loss can cause data loss.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

Spec: `specs/001-spdk-nvme-block-device/spec.md` (cited as "spec").

## BD-1. Only the actor thread uses the controller and the queue pairs

- **Rule:** Use the SPDK controller pointer and the queue pairs only on the actor thread. Send all work to the actor through the client channels.
- **Reason:** `NvmeController` and `QueuePair` are `Send` only because of this rule. SPDK queue pairs are not thread-safe.
- **Applies to:** `src/actor.rs`, `src/controller.rs`, `src/qpair.rs`.
- **Status:** Established by spec (FR-014, FR-020) and by code.
- **Evidence:** `src/controller.rs:116-119` (`// SAFETY:` comment); `src/actor.rs:276`; spec lines 214 and 220.

## BD-2. Keep a DMA buffer alive until the drive completes the command

- **Rule:** Keep the `Arc` of each buffer in its `PendingOp` until SPDK reports the completion. Do not drop a `PendingOp` when the controller can still write into the buffer.
- **Reason:** If the code frees the buffer first, the drive writes into memory that has a different use.
- **Applies to:** abort, time-out, reset, client disconnect and shutdown paths.
- **Status:** Established by spec (FR-005). **Gaps:** the time-out path and the reset path remove the `PendingOp` before the drive completes the command.
- **Evidence:** spec line 205; `src/actor.rs:236-244` (buffers held in `PendingOp`); `src/actor.rs:593-597` (time-out); `src/actor.rs:607-614` (reset).

## BD-3. Flush after each write that must survive a power loss

- **Rule:** If data must survive a power loss, send `FlushSync` after the write completes. Do not assume that a write completion means that the data is on non-volatile media.
- **Reason:** The driver sends all I/O with `io_flags` = 0 (no FUA).
- **Applies to:** `extent-manager` checkpoints, each component that writes metadata.
- **Status:** Established by spec (FR-031) and by code. `extent-manager` sends a flush only with the `volatile_write_cache` feature.
- **Evidence:** spec line 232; `src/actor.rs:1249-1288` (`do_sync_flush`); `components/extent-manager/src/lib.rs:308-310`.

## BD-4. Stop the actor, then detach the controller, then stop SPDK

- **Rule:** At shutdown, call `signal_stop` or `shutdown`, then `detach_controller`. Do this for each drive before `spdk_env` calls `spdk_env_fini`.
- **Reason:** The actor drains the commands in progress for at most 5 s. `detach_controller` frees the queue pairs and calls `spdk_nvme_detach`. Without it, the controller is not freed.
- **Applies to:** `dispatcher::shutdown`, the server applications.
- **Status:** Established by spec (FR-027, FR-028).
- **Evidence:** spec lines 227-228; `src/lib.rs:396-400`; the dispatcher obeys it in `components/dispatcher/src/lib.rs:2165-2186`.

## BD-5. One user for each client channel at a time

- **Rule:** Give each `ClientChannels` from `connect_client()` to one thread at a time.
- **Reason:** The channels are single-producer, single-consumer queues. Two users take the completions of each other, and one of them waits for all time.
- **Applies to:** `dispatcher` (`ChannelPool`), `extent-manager` (`BlockDeviceClient`), each new client.
- **Status:** Established by spec (FR-002) and by code. See DSP-6 in `components/dispatcher/DESIGN-CONSTRAINTS.md`.
- **Evidence:** spec line 202; `src/lib.rs:66-68` (256 slots for each channel; `README.md` and `CLAUDE.md` still state 64).

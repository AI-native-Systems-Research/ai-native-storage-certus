# Feature Specification: GPU P2P DMA Server (`gpu-p2p-server` binary)

**Feature Branch**: `003-gpu-p2p-server`
**Created**: 2026-07-22
**Status**: Draft (backfilled — needs human review)
**Input**: Backfilled from unspecced production code during spec-sync
(drift-report 2026-07-22). Source: `components/gpu-services/src/bin/p2p_server.rs`
(678 lines, `p2p` feature).

> **Backfill note**: This spec was generated automatically from existing code
> to close a documentation gap, not authored before implementation. A human
> should review it against actual product intent (CLI defaults, socket
> protocol stability, error-message contract) before treating it as
> authoritative. It is intentionally distinct from
> `specs/001-gpu-cuda-services/contracts/unix_socket_protocol.md`, which
> documents the Python demo client/server handoff, not this benchmarking
> server.

## Overview

`gpu-p2p-server` is a standalone binary (gated behind the `p2p` feature,
built from `components/gpu-services/src/bin/p2p_server.rs`) that accepts a
base64-encoded CUDA IPC memory handle from a client over a Unix domain
socket and performs an NVMe → GPU VRAM DMA transfer using
`block-device-spdk-nvme`. It exists primarily to benchmark and validate
three different NVMe-to-GPU data paths side by side.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Benchmark NVMe-to-GPU Transfer Modes (Priority: P1)

An engineer wants to compare the latency/throughput of three different
NVMe-to-GPU data paths: a CPU bounce buffer, a pre-pinned GDRCopy P2P
staging pool, and a per-request (cold) GDRCopy P2P path. They start
`gpu-p2p-server` with `--mode <bounce|p2p|p2p-cold>`, connect a client that
sends a CUDA IPC handle for its GPU buffer, and observe the transfer
complete with a success/error response on the socket.

**Why this priority**: This is the sole reason the binary exists — without
mode selection there is nothing to benchmark.

**Independent Test**: Start the server with each of the three `--mode`
values in turn, send an identical IPC-handle payload from a test client for
each, and confirm all three report `OK <size> bytes (<mode>, <n> chunks)`.

**Acceptance Scenarios**:

1. **Given** the server is started with `--mode bounce`, **When** a client
   sends a valid IPC handle payload, **Then** the server reads from NVMe
   into a host DMA buffer, `cudaMemcpy`s host-to-device into the client's
   GPU buffer, and responds `OK <size> bytes (bounce, <n> chunks)`.
2. **Given** the server is started with `--mode p2p`, **When** a client
   connects, **Then** the server reuses its pre-allocated, pre-pinned
   GDRCopy staging pool (allocated once at startup) to perform NVMe →
   staging → device-to-device copy into the client's GPU buffer, and
   responds `OK <size> bytes (p2p, <n> chunks)`.
3. **Given** the server is started with `--mode p2p-cold`, **When** a
   client connects, **Then** the server performs GDRCopy pin/map and unpin
   per request (no amortized setup) and responds
   `OK <size> bytes (p2p-cold, <n> chunks)`.

---

### User Story 2 - Handle One Client Then Exit for Scripted Benchmarks (Priority: P2)

A benchmark harness wants to run the server as a subprocess, have it serve
exactly one client connection, and exit so the harness can capture timing
without manually killing the process.

**Why this priority**: Enables automated, scripted benchmark runs.

**Independent Test**: Start the server with `--once`, connect one client,
verify the server process exits after responding.

**Acceptance Scenarios**:

1. **Given** `--once` is passed, **When** one client connects and the
   transfer completes (success or error), **Then** the server closes the
   listener, removes the socket file, and exits.
2. **Given** `--once` is NOT passed, **When** a client connects and
   disconnects, **Then** the server continues accepting further connections
   until `SIGINT`/`SIGTERM` is received.

---

### User Story 3 - Graceful Shutdown and Cleanup (Priority: P3)

An operator running the server as a long-lived process wants to stop it
cleanly (e.g. `Ctrl-C` or `systemctl stop`) without leaving a stale socket
file or corrupting SPDK/CUDA teardown.

**Why this priority**: Operational hygiene; prevents "address already in
use" on restart and avoids the known SPDK atexit teardown crash.

**Independent Test**: Send `SIGTERM` to a running server process and verify
it exits promptly, removes its socket file, and does not crash during
teardown.

**Acceptance Scenarios**:

1. **Given** the server is running and accepting connections, **When**
   `SIGINT` or `SIGTERM` is received, **Then** the accept loop observes the
   signal flag, breaks out, drops the chunk pool, removes the socket file,
   and the process exits without panicking.
2. **Given** the process is exiting normally, **When** SPDK/CUDA global
   state would otherwise run atexit teardown, **Then** an `atexit` hook
   calls `_exit(0)` directly to avoid a known SPDK teardown crash.

---

### Edge Cases

- What happens when the required kernel modules (`nvidia_peermem`,
  `gdrdrv`) are not loaded? → `initialize_stack` returns an error and the
  process exits with a FATAL message before binding the socket.
- What happens when the client payload is not valid base64, or decodes to
  something other than 72 bytes (64-byte IPC handle + 8-byte LE size)? →
  `parse_client_payload` returns an error, which is written back to the
  client as `ERROR: <message>`.
- What happens when `--mode p2p` is selected but `--staging-size` is
  smaller than `--chunk-size`? → At least one chunk is still allocated
  (`(staging_size + chunk_size - 1) / chunk_size` rounds up).
- What happens if `accept()` would block? → The server sleeps 100
  microseconds and retries (non-blocking listener + polling loop), so it
  can observe the shutdown flag promptly instead of blocking indefinitely
  in `accept()`.
- What happens if a transfer fails mid-flight (e.g. NVMe read error)? → The
  handler function returns `Err`, which is logged to stderr and written to
  the client socket as `ERROR: <message>`; the server does not crash and
  continues (unless `--once`).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The binary MUST be built only when the `p2p` feature is
  enabled (`cargo build -p gpu-services --features p2p --bin gpu-p2p-server`).
- **FR-002**: The binary MUST accept CLI arguments: `--socket <path>`
  (default `/tmp/gpu_p2p_server.sock`), `--pci <addr>` (optional NVMe PCI
  address, uses first discovered device if omitted), `--mode
  <bounce|p2p|p2p-cold>` (default `p2p`), `--staging-size <bytes>` (default
  4194304), `--chunk-size <bytes>` (default 131072, must not exceed NVMe
  MDTS), and `--once` (serve exactly one client then exit).
- **FR-003**: On startup, the binary MUST verify the `nvidia_peermem` and
  `gdrdrv` kernel modules are loaded (via `/proc/modules`) and exit with a
  FATAL error before binding the socket if either is missing.
- **FR-004**: On startup, the binary MUST initialize the SPDK environment,
  the GPU services component (CUDA), and open the target NVMe block device
  (by PCI address or the first discovered device), and install an `atexit`
  hook that calls `_exit(0)` to bypass SPDK's global teardown path.
- **FR-005**: When `--mode p2p` is selected, the binary MUST pre-allocate a
  pool of GDRCopy-pinned, SPDK-registered GPU staging buffers sized to
  cover `--staging-size` in `--chunk-size` increments before accepting any
  connections, so per-request setup cost is amortized out of the
  benchmark.
- **FR-006**: The binary MUST listen on a Unix domain socket at
  `--socket`, removing any pre-existing file at that path first, and MUST
  accept connections via a non-blocking `accept()` loop (polling every 100
  microseconds) so shutdown signals are observed promptly.
- **FR-007**: For each accepted client connection, the binary MUST read a
  single newline-terminated line containing a base64-encoded 72-byte
  payload (64-byte `cudaIpcMemHandle_t` + 8-byte little-endian size),
  decode it, and open the CUDA IPC memory handle to obtain the client's GPU
  device pointer.
- **FR-008**: The binary MUST dispatch the transfer to the handler for the
  configured `--mode`:
  - `bounce`: NVMe → host DMA buffer → `cudaMemcpy` host-to-device → client
    GPU buffer.
  - `p2p`: NVMe → pre-pinned GPU staging buffer (from the pool created in
    FR-005) → device-to-device copy → client GPU buffer.
  - `p2p-cold`: NVMe → per-request GDRCopy pin/map of a staging region →
    device-to-device copy → client GPU buffer → per-request GDRCopy
    unpin/unmap.
- **FR-009**: On success, the binary MUST write a single response line of
  the form `OK <size> bytes (<mode>, <n> chunks)` back to the client
  socket. On failure at any stage, it MUST write `ERROR: <message>` to the
  client socket and log the error to stderr, without crashing the server
  process.
- **FR-010**: When `--once` is passed, the binary MUST serve exactly one
  client connection (success or error) and then exit, removing the socket
  file. When `--once` is not passed, the binary MUST continue accepting
  further connections until a shutdown signal is received.
- **FR-011**: The binary MUST install `SIGINT` and `SIGTERM` handlers that
  set an atomic shutdown flag; the accept loop MUST check this flag on
  every iteration and, when set, break out, drop any GPU staging pool
  (releasing GDRCopy/SPDK resources), remove the socket file, and exit.
- **FR-012**: All transfer handlers MUST perform NVMe reads in
  `--chunk-size` increments (not exceeding the NVMe controller's MDTS).

### Key Entities

- **TransferMode**: One of `Bounce`, `P2p`, `P2pCold` — selects which
  NVMe-to-GPU data path a connection is served with.
- **ServerContext**: Holds the opened NVMe block device handle, SPDK
  environment handle, GPU services component, logger, sector size, and
  namespace ID — constructed once at startup.
- **GpuStagingBuffer**: A single pre-pinned (GDRCopy) GPU buffer registered
  with SPDK, reused across requests in `p2p` mode.
- **ChunkPool**: A collection of `GpuStagingBuffer`s sized to
  `--staging-size` in `--chunk-size` increments, allocated once for `p2p`
  mode.
- **Client wire protocol**: One line in (base64 72-byte IPC payload), one
  line out (`OK ...` or `ERROR: ...`) — deliberately simple, distinct from
  the length-prefixed protocol in
  `specs/001-gpu-cuda-services/contracts/unix_socket_protocol.md`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The server successfully completes a transfer end-to-end
  (client connect → response line) in each of the three modes given a
  valid IPC handle payload.
- **SC-002**: `--once` reliably serves exactly one client and exits,
  suitable for scripted benchmark harnesses invoking the binary
  per-iteration.
- **SC-003**: `SIGINT`/`SIGTERM` result in clean shutdown (socket file
  removed, no panic, no crash during SPDK/CUDA teardown) within one polling
  interval (~100 microseconds) of signal delivery reaching the accept loop.
- **SC-004**: Malformed client payloads (bad base64, wrong length, stale
  IPC handle) never crash the server; they always produce an `ERROR: ...`
  response.

## Assumptions

- This binary is a benchmarking/validation tool, not a public API; its
  wire protocol (single base64 line in, single text line out) is not
  guaranteed stable and is not the same protocol as the
  `apps/gpu-handle-test-server` demo covered by spec 001.
- The `p2p` feature and this binary depend on `block-device-spdk-nvme`,
  `spdk-env`, GDRCopy (`gpu-services` `dma::create_spdk_dma_buffer_from_gpu_bar`),
  and the `nvidia_peermem`/`gdrdrv` kernel modules being present on the host.
- Exactly one NVMe controller and one GPU are exercised per server
  instance; multi-device fan-out is out of scope.
- This spec documents behavior as implemented at backfill time
  (2026-07-22); it has not been reviewed against original design intent by
  a human author.

# Feature Specification: RDMA Network Test Tool

**Feature Branch**: `001-rdma-network-test`

**Created**: 2026-06-05

**Status**: Draft

**Input**: User description: "Create a RDMA network test program in Rust, that allows one to measure throughput and latency/jitter across the network. The program should select between client and server nodes with a command line parameter. You should create a script to launch client/server pairs using ssh to perform a remote launch. The program should use ibverbs and check its availability."

## Clarifications

### Session 2026-06-05

- Q: Should results be available in machine-parseable format for CI integration? → A: Human-readable output by default, with JSON output available via `--output json` flag.
- Q: What happens when the RDMA connection is interrupted mid-test? → A: Retry connection up to 3 times, then abort and report partial results collected before failure.
- Q: Should the server support multiple clients or multi-stream testing? → A: Single client only (1:1 point-to-point); server exits after one session completes.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Measure RDMA Throughput Between Two Nodes (Priority: P1)

A network engineer wants to measure the maximum RDMA throughput between two nodes to validate fabric performance. They run the tool in server mode on one machine and client mode on the other, specifying a throughput test. The tool performs bulk one-sided RDMA Write operations and reports sustained bandwidth.

**Why this priority**: Throughput is the primary metric for validating RDMA fabric health and is the most common reason to run a network benchmark.

**Independent Test**: Can be fully tested by launching the server on one RDMA-capable host and the client on another, running the throughput test, and verifying that bandwidth results are produced and within expected hardware limits.

**Acceptance Scenarios**:

1. **Given** two nodes with RDMA-capable NICs on the same fabric, **When** the user runs the tool in server mode on node A and client mode on node B with `--test throughput`, **Then** the tool reports bandwidth in GB/s, message rate, and total data transferred.
2. **Given** the user specifies a custom message size (e.g., 65536 bytes), **When** the throughput test completes, **Then** the results reflect the configured message size and corresponding bandwidth.
3. **Given** the user specifies a number of iterations, **When** the test runs, **Then** exactly that number of RDMA Write operations are performed during measurement (excluding warmup).

---

### User Story 2 - Measure RDMA Latency and Jitter (Priority: P1)

A network engineer wants to measure one-way latency and jitter between two RDMA nodes to evaluate network quality for latency-sensitive workloads. The tool performs a Send/Recv ping-pong pattern and computes statistical latency metrics.

**Why this priority**: Latency and jitter are critical for real-time and inferencing workloads; equally important as throughput for the target use case.

**Independent Test**: Can be tested by running client/server, executing the latency test, and verifying statistical output (min, max, mean, percentiles, jitter/stddev) is produced.

**Acceptance Scenarios**:

1. **Given** two connected RDMA nodes, **When** the user runs the latency test, **Then** the tool reports min, max, mean, median, P95, P99 latency and jitter (standard deviation).
2. **Given** the user configures a small message size (e.g., 64 bytes), **When** the latency test runs, **Then** results reflect the overhead characteristics of small-message RDMA operations.
3. **Given** warmup iterations are configured, **When** the test runs, **Then** warmup iterations are excluded from the reported statistics.

---

### User Story 3 - Launch Tests Remotely via SSH Script (Priority: P2)

A network engineer wants to quickly launch a client/server test pair across two remote nodes without manually SSHing into each machine. They use a launch script that handles server startup, client execution, and result collection.

**Why this priority**: Automation reduces operator burden and enables integration into CI/testing pipelines, but requires the core tool to exist first.

**Independent Test**: Can be tested by providing two SSH-accessible hostnames and verifying the script launches the server, runs the client, collects output, and cleans up.

**Acceptance Scenarios**:

1. **Given** two hostnames with SSH key-based access, **When** the user runs the launch script with server and client host arguments, **Then** the server is started on the first host, the client runs against it on the second host, and results are displayed.
2. **Given** the server fails to start (e.g., no RDMA device), **When** the script detects the failure, **Then** it reports an error and does not attempt to launch the client.
3. **Given** additional options are passed to the script, **When** the test runs, **Then** those options are forwarded to both the server and client processes.

---

### User Story 4 - Verify RDMA/ibverbs Availability (Priority: P2)

A user runs the tool on a machine where RDMA may not be properly configured. The tool checks for ibverbs library availability and RDMA device presence before attempting any RDMA operations, providing clear diagnostic messages if something is missing.

**Why this priority**: Graceful failure with actionable diagnostics prevents frustration and reduces support burden when the tool is used across diverse environments.

**Independent Test**: Can be tested on a machine without RDMA hardware/drivers and verifying that the tool exits cleanly with an informative error message.

**Acceptance Scenarios**:

1. **Given** a machine without libibverbs installed, **When** the tool starts, **Then** it reports that ibverbs is not available and suggests installation steps.
2. **Given** a machine with libibverbs but no RDMA devices, **When** the tool starts, **Then** it reports no devices found and suggests loading drivers or configuring SoftRoCE.
3. **Given** a machine with working RDMA, **When** the tool starts, **Then** it lists detected devices with their type (InfiniBand or RoCE) and port state.

---

### Edge Cases

- When the RDMA connection is interrupted mid-test, the tool retries up to 3 times. If all retries fail, it aborts and reports partial results collected before failure.
- When the specified RDMA device does not exist, the tool reports an error listing available devices.
- When the server is not yet ready when the client connects, the client retries connection (covered by the 3-retry policy).
- When the user specifies zero iterations or zero message size, the tool rejects the input with a validation error at startup.
- Message size is configured on the client and communicated to the server during connection setup; mismatches are not possible.
- The server handles exactly one client session, then exits. No concurrent client support.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST operate in either server (listener) or client (connector) mode, selected via a command-line subcommand.
- **FR-002**: System MUST perform RDMA Write operations for throughput measurement and report bandwidth (GB/s), message rate (Mmsg/s), and total data transferred.
- **FR-003**: System MUST perform Send/Recv ping-pong for latency measurement and report min, max, mean, median, P95, P99 latency, and jitter (standard deviation).
- **FR-004**: System MUST check for ibverbs library presence and RDMA device availability at startup, providing actionable diagnostic messages if either is missing.
- **FR-005**: System MUST allow configuration of message size, iteration count, warmup count, test type, port, output format, and optionally the RDMA device name via command-line parameters.
- **FR-006**: System MUST support both InfiniBand and RoCE transports via auto-detection of available RDMA devices.
- **FR-007**: System MUST include a launch script that starts a server on one remote host and a client on another using SSH, collecting and displaying results.
- **FR-008**: System MUST perform configurable warmup iterations before measurement to avoid cold-start effects.
- **FR-009**: System MUST use the ibverbs API (libibverbs) for all RDMA operations.
- **FR-010**: System MUST exit with a non-zero status code and clear error message when RDMA operations fail.
- **FR-011**: System MUST support output in both human-readable (default) and JSON formats, selectable via `--output json` flag.
- **FR-012**: System MUST retry failed RDMA connections up to 3 times before aborting, and report partial results if any measurements were collected before failure.
- **FR-013**: System server MUST handle exactly one client session and exit upon completion; no multi-client or concurrent stream support.

## Implementation Details

### FR-007: SSH Launch Script

**Location**: `tools/rdma-test/scripts/launch.sh` (106 lines)

**Status**: ✅ Production-ready

**Synopsis**: Automates remote server/client launch across two SSH-accessible hosts without manual terminal management.

**Usage**:
```bash
./scripts/launch.sh <server-host> <client-host> [options]
```

**Arguments**:
- `<server-host>`: Hostname or IP of server node (SSH-accessible)
- `<client-host>`: Hostname or IP of client node (SSH-accessible)
- `[options]`: Arbitrary options forwarded to both server and client (e.g., `--test throughput --message-size 65536 --iterations 10000`)

**Environment Variables**:
- `RDMA_TEST_BIN`: Path to compiled rdma-test binary (default: `./target/release/rdma-test`)
- `RDMA_TEST_PORT`: Server listen port (default: `50000`)
- `RDMA_TEST_STARTUP_DELAY`: Delay after server launch before client starts (seconds, default: `2`)

**Behavior**:
1. Starts server process on `<server-host>` in background
2. Waits for server to be ready (health check with timeout)
3. Spawns client on `<client-host>` connected to `<server-host>`
4. Collects and displays client output
5. Cleans up on exit (kills remote server, trap handler for signals)

**Error Handling**:
- If server fails to start, script reports error and exits without launching client
- Connection failures are logged with diagnostic info
- Both stdout and stderr from remote processes are captured and displayed

**Example Invocations**:
```bash
# Throughput test with default settings
./scripts/launch.sh node1.example.com node2.example.com --test throughput

# Latency test with custom message size and warmup
./scripts/launch.sh node1 node2 --test latency --message-size 1024 --warmup 100

# Custom iterations and JSON output
./scripts/launch.sh node1 node2 --iterations 50000 --output json
```

### Key Entities

- **RDMA Device**: A network interface card capable of RDMA operations, identified by name (e.g., mlx5_0), with attributes including transport type (IB/RoCE) and port state.
- **Test Session**: A single client-server connection over which benchmarks are executed, characterized by message size, iteration count, and test type. Exactly one session per server invocation.
- **Measurement Result**: The output of a benchmark run, containing throughput metrics (bandwidth, message rate) or latency metrics (statistical distribution). Serializable to both human-readable text and JSON.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Users can measure point-to-point RDMA throughput between any two RDMA-capable nodes in under 30 seconds for a standard 10,000-iteration test.
- **SC-002**: Latency measurements are accurate to sub-microsecond resolution, consistent with results from standard RDMA benchmarking tools (e.g., ib_write_lat).
- **SC-003**: The tool provides clear, actionable diagnostics when RDMA is not available, enabling a user to resolve the issue without external documentation in 90% of common failure cases.
- **SC-004**: The SSH launch script reduces time-to-first-result from manual two-terminal setup (typically 60+ seconds) to a single command completing in under 45 seconds.
- **SC-005**: The tool operates correctly on both InfiniBand and RoCE fabrics without manual transport configuration.
- **SC-006**: JSON output is parseable by standard tools (jq, Python json module) without transformation, enabling direct integration into CI result collection.

## Assumptions

- Target systems run Linux with RDMA-capable NICs (InfiniBand HCAs or RoCE-capable Ethernet NICs).
- The rdma-core package (or equivalent vendor OFED) is available for installation on target systems.
- SSH key-based authentication is pre-configured between the operator's machine and both test nodes (for the launch script).
- The tool binary is available at the same filesystem path on both client and server nodes when using the launch script.
- The tool is intended for internal network performance validation, not as a production monitoring service.
- Only Reliable Connection (RC) queue pairs are needed for this version; other transport types (UD, UC) are out of scope.
- Single-stream point-to-point testing is sufficient for v1; multi-stream and multi-client support are out of scope.

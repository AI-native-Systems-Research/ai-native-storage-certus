# Feature Specification: Logger Component

**Feature Branch**: `logger`
**Created**: 2026-04-17
**Status**: Draft
**Input**: User description: "Build a logging component that provides logging
to the console or file with configurable log levels, timestamp formatting,
and colorized console output."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Console Logging with Log Levels (Priority: P1)

A developer integrates the LoggerComponentV1 into their Certus application
and uses the ILogger interface to emit log messages at different severity
levels. Log output goes to the console (stderr) by default, with each
message showing a timestamp, the log level, and the message text. Color
coding distinguishes severity levels at a glance.

**Why this priority**: Console logging is the primary use case and the
default behavior. Every user needs this before any other feature.

**Independent Test**: Create a LoggerComponentV1, call each log method
(error, warn, info, debug), and verify that messages appear on stderr
with correct format and filtering based on RUST_LOG.

**Acceptance Scenarios**:

1. **Given** a LoggerComponentV1 with default configuration and
   RUST_LOG=info, **When** the developer calls `info("server started")`,
   **Then** stderr shows a line with timestamp, "INFO", and "server
   started" in green colorization.
2. **Given** RUST_LOG=warn, **When** the developer calls
   `debug("cache hit")`, **Then** no output appears because debug is
   below the warn threshold.
3. **Given** RUST_LOG=error, **When** the developer calls
   `error("disk failure")`, **Then** stderr shows a line with timestamp,
   "ERROR", and "disk failure" in red colorization.
4. **Given** RUST_LOG is not set, **When** the developer calls any log
   method, **Then** a reasonable default level is applied (info or above).

---

### User Story 2 - File-Based Logging (Priority: P2)

A developer configures the LoggerComponentV1 to write log output to a
file instead of the console. The file output contains the same timestamp,
log level, and message format but without color codes. Log level
filtering via RUST_LOG still applies.

**Why this priority**: File logging is essential for production
deployments where console output is not captured, but console logging
must work first.

**Independent Test**: Configure LoggerComponentV1 for file output,
emit log messages, and verify the file contains correctly formatted
entries without ANSI color codes.

**Acceptance Scenarios**:

1. **Given** a LoggerComponentV1 configured to write to "/tmp/app.log",
   **When** the developer calls `warn("high latency detected")`,
   **Then** the file contains a line with timestamp, "WARN", and the
   message text, with no ANSI escape sequences.
2. **Given** a LoggerComponentV1 configured for file output and
   RUST_LOG=debug, **When** the developer calls `debug("trace point")`,
   **Then** the message appears in the log file.
3. **Given** the specified log file path does not exist, **When** the
   LoggerComponentV1 is configured for that path, **Then** the file is
   created automatically.

---

### User Story 3 - Component Integration via ILogger (Priority: P3)

Another Certus component (e.g., extent-manager) declares an ILogger
receptacle, and the LoggerComponentV1 is bound to it. The consuming
component uses the ILogger interface for structured logging without
depending on the logger implementation.

**Why this priority**: Integration with the component framework is the
ultimate purpose, but the logging functionality must work standalone
first.

**Independent Test**: Create a LoggerComponentV1, query it for ILogger
via IUnknown, and bind it to a test component's receptacle. Call log
methods through the receptacle and verify output.

**Acceptance Scenarios**:

1. **Given** a LoggerComponentV1, **When** `query_interface!(component,
   ILogger)` is called, **Then** it returns a valid ILogger reference.
2. **Given** a component with an ILogger receptacle, **When** the
   LoggerComponentV1 is bound via `connect_receptacle`, **Then** the
   receptacle's `get()` returns the ILogger implementation.
3. **Given** a bound ILogger receptacle, **When** the consuming component
   calls `logger.info("operation complete")`, **Then** the message is
   logged to the configured output (console or file).

---

### Edge Cases

- What happens when RUST_LOG contains an invalid value? The logger
  falls back to a default level (info).
- What happens when the log file cannot be opened (permission denied)?
  The logger returns an error during configuration.
- What happens when multiple threads log concurrently? All messages
  are written without interleaving (thread-safe output).
- What happens when RUST_LOG is set to "trace"? The logger recognizes
  it even though the ILogger interface exposes debug as the finest
  level.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The component MUST be named LoggerComponentV1 and defined
  using the `define_component!` macro.
- **FR-002**: The ILogger interface MUST be defined in the shared
  interfaces crate using the `define_interface!` macro, with methods
  for error, warn, info, and debug log levels.
- **FR-003**: The component MUST implement IUnknown for runtime
  interface discovery of ILogger.
- **FR-004**: Log level filtering MUST be controlled by the RUST_LOG
  environment variable, following the same semantics as the env_logger
  crate (e.g., "debug", "info", "warn", "error").
- **FR-005**: Console output MUST be the default logging destination,
  writing to stderr.
- **FR-006**: Console output MUST include ANSI color codes to
  distinguish log levels (e.g., red for error, yellow for warn, green
  for info, blue/cyan for debug).
- **FR-007**: The component MUST support an alternative file output
  mode where log messages are written to a specified file path.
- **FR-008**: File output MUST NOT contain ANSI color codes.
- **FR-009**: Every log message MUST include a timestamp, log level,
  and the user-provided message text.
- **FR-010**: The component MUST be thread-safe (Send + Sync), allowing
  concurrent logging from multiple threads without message interleaving.
- **FR-011**: When RUST_LOG is not set, the logger MUST default to a
  reasonable log level (info).

### Key Entities

- **LoggerComponentV1**: The component implementing ILogger, created
  via `define_component!`. Manages output destination and log filtering.
- **ILogger**: The interface trait defined via `define_interface!` in
  the interfaces crate. Provides error(), warn(), info(), debug()
  methods.
- **Log Level**: Severity classification (Error, Warn, Info, Debug)
  parsed from RUST_LOG environment variable.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All four log methods (error, warn, info, debug) produce
  correctly formatted output containing timestamp, level, and message.
- **SC-002**: Log level filtering correctly suppresses messages below
  the configured threshold for all four levels.
- **SC-003**: Console output displays distinct colors for each log
  level when connected to a terminal.
- **SC-004**: File output contains zero ANSI escape sequences.
- **SC-005**: Concurrent logging from 4+ threads produces no
  interleaved or corrupted messages.
- **SC-006**: The component passes IUnknown query for ILogger and binds
  successfully to receptacles in other components.
- **SC-007**: All public APIs have doc tests with runnable examples and
  Criterion benchmarks for performance-sensitive operations.

## Assumptions

- The RUST_LOG environment variable follows env_logger parsing
  conventions (level names are case-insensitive, "trace" maps to the
  debug level since ILogger does not expose a separate trace method).
- Console output targets stderr, not stdout, consistent with standard
  logging practice.
- File output uses append mode, creating the file if it does not exist.
- The timestamp format uses ISO 8601 (e.g., "2026-04-17T14:30:00.123Z")
  for unambiguous machine-parseable output.
- Color detection for console output relies on checking whether stderr
  is connected to a terminal (TTY detection).
- The component does not implement log rotation; that is out of scope
  for v1.
- Performance benchmarks target log message formatting and emission
  throughput, not I/O subsystem performance.

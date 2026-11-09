# Feature Specification: Generic Log Handler

**Feature Branch**: `006-log-handler`
**Created**: 2026-04-01
**Status**: Backfilled
**Source**: Generated from existing implementation via `speckit.sync.propose`

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.propose`.
> It documents current behavior. Review and update to reflect desired behavior.

## User Scenarios & Testing

### User Story 1 - Actor-Based Logging (Priority: P1)

A framework user creates an actor with the built-in `LogHandler` to
receive structured log messages. The handler writes timestamped,
level-tagged lines to stderr and optionally to a file. This eliminates
the need to define a custom logging handler for common use cases.

**Why this priority**: Logging is the most common cross-cutting concern
in any component system. A reusable handler saves every user from
reimplementing the same pattern.

**Independent Test**: Create a `LogHandler` actor, send messages at
each level, deactivate, and verify output on stderr and optional file.

**Acceptance Scenarios**:

1. **Given** an actor using `LogHandler::new()`, **When** log messages
   at various levels are sent, **Then** each message appears on stderr
   with a timestamp and level tag.
2. **Given** an actor using `LogHandler::with_file(path)`, **When** log
   messages are sent, **Then** messages appear on both stderr and the
   specified file.
3. **Given** a `LogHandler` configured with `with_min_level(Warn)`,
   **When** Debug and Info messages are sent, **Then** they are silently
   discarded; only Warn and Error messages appear in output.
4. **Given** a `LogHandler` with file output, **When** the actor is
   deactivated, **Then** the file is flushed and contains all messages
   at or above the minimum level.

---

### User Story 2 - Structured Log Messages (Priority: P1)

A framework user creates log messages using convenience constructors
(`LogMessage::info(text)`, `LogMessage::warn(text)`, etc.) that pair a
severity level with a text payload.

**Why this priority**: Log messages are the data type flowing through
the logging actor. Without them, the handler has nothing to process.

**Independent Test**: Create `LogMessage` values using each constructor
and verify level and text fields.

**Acceptance Scenarios**:

1. **Given** the `LogMessage` constructors, **When**
   `LogMessage::info("hello")` is called, **Then** a message with
   level `Info` and text `"hello"` is created.
2. **Given** `LogLevel` variants, **When** compared, **Then**
   `Debug < Info < Warn < Error` holds.
3. **Given** a `LogLevel` value, **When** formatted with `Display`,
   **Then** a 5-character padded string is produced (e.g., `"INFO "`).

---

### Edge Cases

- What happens when the file path is invalid or the directory does not
  exist? `LogHandler::with_file()` returns an `io::Error`.
- What happens when the handler receives a message below the minimum
  level? The message is silently discarded (no output, no error).
- What happens when stderr is not available? Standard library panics
  apply; this is not handled specially.

## Requirements

### Functional Requirements

- **FR-001**: The framework MUST provide a `LogLevel` enum with
  `Debug`, `Info`, `Warn`, `Error` variants, ordered by severity
  (`Debug < Info < Warn < Error`).
- **FR-002**: The framework MUST provide a `LogMessage` struct
  containing a level and text, plus convenience constructors
  `debug()`, `info()`, `warn()`, `error()`.
- **FR-003**: The framework MUST provide a `LogHandler` implementing
  `ActorHandler<LogMessage>` that writes timestamped log lines to
  stderr.
- **FR-004**: `LogHandler` MUST support optional file output via
  `with_file(path)`, appending to the file with buffered writes.
- **FR-005**: `LogHandler` MUST support minimum level filtering via
  `with_min_level(level)`. Messages below the threshold are discarded.
- **FR-006**: `LogHandler` MUST flush file buffers on actor shutdown
  (via `on_stop`).
- **FR-007**: Log line format MUST include an ISO-8601 timestamp and
  a 5-character padded level tag (e.g.,
  `2026-04-01T14:23:05.123Z [INFO ] message text`).
- **FR-008**: The timestamp MUST be generated from `std::time::SystemTime`
  without external dependencies.

### Key Entities

- **LogLevel**: Severity enum (`Debug`, `Info`, `Warn`, `Error`).
  Implements `Display` with 5-char padding for aligned output.
- **LogMessage**: A structured log entry pairing a `LogLevel` with a
  `String` payload. Provides `level()` and `text()` accessors.
- **LogHandler**: An `ActorHandler<LogMessage>` that formats and writes
  log lines. Supports stderr-only or stderr+file dual output, with
  configurable minimum level filtering.

## Success Criteria

### Measurable Outcomes

- **SC-001**: All public types (`LogLevel`, `LogMessage`, `LogHandler`)
  have doc tests and unit tests.
- **SC-002**: Level filtering correctly suppresses messages below the
  configured threshold — verified by writing to a temp file and
  checking contents.
- **SC-003**: File output contains all messages at or above the minimum
  level after actor deactivation (BufWriter flushed).
- **SC-004**: `LogHandler::default()` is equivalent to
  `LogHandler::new()` (stderr-only, min level `Debug`).
- **SC-005**: The framework includes a runnable `actor_log` example
  demonstrating stderr-only, file output, and level filtering.

## Assumptions

- The log handler is a synchronous, blocking implementation. Async
  logging is out of scope.
- No external logging crate dependencies (e.g., `log`, `tracing`).
  The handler is self-contained.
- File output uses append mode with `BufWriter` for performance.
- The timestamp formatter is hand-rolled (no `chrono` dependency),
  producing RFC-3339/ISO-8601 format.
- This feature builds on spec 003 (actor-channels) for the
  `ActorHandler` trait and `Actor` type.

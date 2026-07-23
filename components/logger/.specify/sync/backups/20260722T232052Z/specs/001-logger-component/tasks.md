# Tasks: Logger Component

**Input**: Design documents from `specs/001-logger-component/`
**Prerequisites**: plan.md (required), spec.md (required), research.md,
data-model.md, contracts/ilogger.md

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)
- Exact file paths included in all descriptions

---

## Phase 1: Setup

**Purpose**: Project initialization and workspace integration

- [x] T001 Add `logger` to workspace `Cargo.toml` at `/home/dwaddington/ai-native-storage-certus/Cargo.toml`: add `"components/logger/v1"` to both `members` and `default-members` arrays, and add `logger = { path = "components/logger/v1" }` to `[workspace.dependencies]`
- [x] T002 Create `Cargo.toml` at `/home/dwaddington/ai-native-storage-certus/components/logger/v1/Cargo.toml` with package name `logger`, version `0.1.0`, `edition.workspace = true`, `rust-version.workspace = true`, `publish = false`, dependencies on `component-framework.workspace`, `component-core.workspace`, `component-macros.workspace`, `interfaces.workspace`, `chrono` (with `clock` feature), and dev-dependencies on `criterion` with `[[bench]]` entry for `log_throughput`
- [x] T003 [P] Create directory structure: `src/`, `benches/`, `tests/` under `/home/dwaddington/ai-native-storage-certus/components/logger/v1/`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and interface definition that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Define `ILogger` interface in `/home/dwaddington/ai-native-storage-certus/components/interfaces/src/ilogger.rs` using `define_interface!` macro with methods: `fn error(&self, msg: &str)`, `fn warn(&self, msg: &str)`, `fn info(&self, msg: &str)`, `fn debug(&self, msg: &str)`. Follow the same pattern as `igreeter.rs`
- [x] T005 Export `ILogger` from `/home/dwaddington/ai-native-storage-certus/components/interfaces/src/lib.rs`: add `mod ilogger;` and `pub use ilogger::ILogger;`
- [x] T006 Implement `LogLevel` enum in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs` with variants `Error`, `Warn`, `Info`, `Debug`, numeric ordering, `Display` impl (5-char padded uppercase), and `from_env()` function that reads `RUST_LOG`, parses case-insensitively, maps "trace" to Debug, defaults to Info on missing/invalid values
- [x] T007 Implement ANSI color helper in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs`: function `colorize(level: &LogLevel, text: &str) -> String` that wraps text in ANSI escape codes (red=`\x1b[31m` for Error, yellow=`\x1b[33m` for Warn, green=`\x1b[32m` for Info, cyan=`\x1b[36m` for Debug, reset=`\x1b[0m`)
- [x] T008 Implement `LoggerComponentV1` struct and `define_component!` in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs`: fields `writer: Mutex<Box<dyn Write + Send>>`, `level: LogLevel`, `use_color: bool`. Use `define_component!` macro with `version: "0.1.0"` and `provides: [ILogger]`. Implement constructor `new() -> Arc<Self>` that creates a stderr writer, detects TTY via `libc::isatty(2)`, and reads RUST_LOG

**Checkpoint**: Foundation ready — ILogger defined, LogLevel parsing works, component skeleton compiles

---

## Phase 3: User Story 1 — Console Logging with Log Levels (Priority: P1)

**Goal**: Developers can use ILogger to emit log messages to stderr with
timestamps, colored level indicators, and RUST_LOG-based filtering

**Independent Test**: Create LoggerComponentV1, call each log method,
verify output format and level filtering

### Implementation for User Story 1

- [x] T009 [US1] Implement `ILogger` trait for `LoggerComponentV1` in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs`: each method (error, warn, info, debug) checks level threshold, formats line as `{chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)} {LEVEL} {msg}\n` with optional ANSI color wrapping the level, acquires mutex, writes to output, flushes
- [x] T010 [US1] Add unit tests in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs` (inline `#[cfg(test)] mod tests`): test LogLevel ordering, test `from_env()` parsing for all level names including "trace" and invalid values, test log format output by creating a logger with a `Vec<u8>` writer (inject via internal constructor) and verifying output contains timestamp pattern, level string, and message
- [x] T011 [US1] Add unit tests for color output: test that console logger with `use_color=true` includes ANSI escape codes in output, and `use_color=false` does not
- [x] T012 [US1] Add doc tests with runnable examples on all public types and methods in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs`: `LoggerComponentV1::new()` example showing console usage, `LogLevel` enum documentation, module-level doc example showing basic usage pattern

**Checkpoint**: Console logging fully functional — `cargo test -p logger` passes, `cargo doc -p logger --no-deps` is warning-free

---

## Phase 4: User Story 2 — File-Based Logging (Priority: P2)

**Goal**: Developers can configure LoggerComponentV1 to write logs to
a file instead of stderr, with same format but no ANSI codes

**Independent Test**: Create file logger, emit messages, verify file
contains correctly formatted entries without ANSI escape sequences

### Implementation for User Story 2

- [x] T013 [US2] Implement `LoggerComponentV1::new_with_file(path: &str) -> io::Result<Arc<Self>>` in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs`: opens file in append+create mode, sets `use_color = false`, reads RUST_LOG for level
- [x] T014 [US2] Add unit tests for file logging in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs`: test that `new_with_file` creates the file, test that output contains no ANSI escape sequences (`\x1b[` pattern absent), test that log level filtering works with file output, test error case when file path is in a non-existent directory
- [x] T015 [US2] Add doc test for `new_with_file` in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/src/lib.rs` showing file logging usage with cleanup

**Checkpoint**: File logging works independently — all tests pass

---

## Phase 5: User Story 3 — Component Integration via ILogger (Priority: P3)

**Goal**: LoggerComponentV1 can be queried for ILogger via IUnknown and
bound to other components' receptacles

**Independent Test**: Create component, query ILogger interface, bind
to a test component's receptacle, verify logging through the receptacle

### Implementation for User Story 3

- [x] T016 [US3] Add integration test in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/tests/integration.rs`: create LoggerComponentV1, call `query_interface!(component, ILogger)` and verify it returns `Some`, call log methods through the queried interface and verify output
- [x] T017 [US3] Add integration test for receptacle binding in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/tests/integration.rs`: define a test component with `define_component!` that has `receptacles: { logger: ILogger }`, create LoggerComponentV1, bind via `connect_receptacle_raw("logger", &*logger_comp)`, verify receptacle `get()` succeeds, call log methods through the receptacle
- [x] T018 [US3] Add thread safety integration test in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/tests/integration.rs`: spawn 4+ threads, each logging 100 messages through the same ILogger reference, verify all messages appear in output without interleaving (each line is complete)

**Checkpoint**: Component integration verified — IUnknown query, receptacle binding, and concurrent access all tested

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, benchmarks, and final quality gates

- [x] T019 [P] Create Criterion benchmark in `/home/dwaddington/ai-native-storage-certus/components/logger/v1/benches/log_throughput.rs`: benchmark log formatting throughput (message formatting + write to `Vec<u8>` sink), benchmark with level filtering (messages below threshold), benchmark with 4 concurrent threads. Use `criterion_group!` and `criterion_main!` macros following the pattern in `components/component-framework/crates/component-framework/benches/method_dispatch.rs`
- [x] T020 [P] Create README.md at `/home/dwaddington/ai-native-storage-certus/components/logger/v1/README.md` following the pattern of `components/example-helloworld/README.md`: describe the component, its ILogger interface, public API (new, new_with_file, log methods), build instructions (`cargo build -p logger`), test instructions (`cargo test -p logger`), benchmark instructions (`cargo bench -p logger`), usage examples for console and file logging, environment variables (RUST_LOG)
- [x] T021 Run full CI gate: `cargo fmt -p logger --check && cargo clippy -p logger -- -D warnings && cargo test -p logger && cargo doc -p logger --no-deps && cargo bench -p logger --no-run`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup (T001-T003)
  — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational (T004-T008)
- **US2 (Phase 4)**: Depends on Foundational (T004-T008);
  independent of US1
- **US3 (Phase 5)**: Depends on Foundational (T004-T008);
  independent of US1/US2
- **Polish (Phase 6)**: Depends on all user stories complete

### Within Each Phase

- T001 before T002 (workspace must exist before crate Cargo.toml)
- T002 before T003 (Cargo.toml before source dirs)
- T004 before T005 (define interface before exporting it)
- T005 before T006 (ILogger must be importable)
- T006, T007 can run in parallel (independent code within lib.rs)
- T008 depends on T006 + T007 (component uses LogLevel + colorize)
- T009 depends on T008 (ILogger impl needs component struct)
- T010, T011 depend on T009 (tests need the impl)
- T012 depends on T009 (doc tests need the impl)
- T013 depends on T008 (file constructor extends component)
- T019, T020 can run in parallel (different files)
- T021 depends on all prior tasks

### Parallel Opportunities

```text
# Phase 2 parallel group:
T006 + T007 (LogLevel enum + color helper — independent functions)

# Phase 3 parallel group (after T009):
T010 + T011 + T012 (all test/doc tasks for US1)

# Phase 4 parallel group (after T013):
T014 + T015 (test + doc tasks for US2)

# Phase 5 parallel group:
T016 + T017 + T018 (all integration tests — same file but
independent test functions)

# Phase 6 parallel group:
T019 + T020 (benchmark + README — different files)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T003)
2. Complete Phase 2: Foundational (T004-T008)
3. Complete Phase 3: User Story 1 (T009-T012)
4. **STOP and VALIDATE**: `cargo test -p logger` passes, console
   logging works
5. Deploy/demo if ready

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. Add US1 → Console logging works (MVP)
3. Add US2 → File logging works
4. Add US3 → Component integration verified
5. Polish → Benchmarks, README, CI gate

### Sequential Single-Developer Flow

T001 → T002 → T003 → T004 → T005 → T006 ∥ T007 → T008 →
T009 → T010 ∥ T011 ∥ T012 → T013 → T014 ∥ T015 →
T016 ∥ T017 ∥ T018 → T019 ∥ T020 → T021

# Data Model: Logger Component

**Date**: 2026-04-17

## Entities

### LogLevel (enum)

Severity levels for log messages, ordered by increasing verbosity.

| Variant | Numeric | Display | Color (console) |
|---------|---------|---------|-----------------|
| Error   | 0       | "ERROR" | Red (\x1b[31m)  |
| Warn    | 1       | "WARN " | Orange (\x1b[38;5;208m) |
| Info    | 2       | "INFO " | Green (\x1b[32m) |
| Debug   | 3       | "DEBUG" | Cyan (\x1b[36m) |

**Parsing**: From `RUST_LOG` env var, case-insensitive. "trace" maps
to Debug. Invalid/missing defaults to Info.

**Filtering rule**: A message is emitted if its level's numeric value
<= the configured threshold's numeric value.

### ILogger (interface trait)

Defined in `interfaces` crate via `define_interface!`.

| Method | Signature | Description |
|--------|-----------|-------------|
| error  | `fn error(&self, msg: &str)` | Log at Error level |
| warn   | `fn warn(&self, msg: &str)`  | Log at Warn level |
| info   | `fn info(&self, msg: &str)`  | Log at Info level |
| debug  | `fn debug(&self, msg: &str)` | Log at Debug level |

### LoggerComponent (component)

Defined via `define_component!`. Provides: `[ILogger]`. No receptacles.

| Field | Type | Description |
|-------|------|-------------|
| writer | `Mutex<Box<dyn Write + Send>>` | Output destination (stderr or File) |
| level | `LogLevel` | Configured threshold from RUST_LOG |
| use_color | `bool` | Whether to emit ANSI codes (TTY detection) |

**Construction**:
- `LoggerComponent::new_default()` → console logger (stderr), color if TTY
- `LoggerComponent::new_with_file(path: &str) -> io::Result<Arc<Self>>`
  → file logger, no color

**Lifecycle**: Immutable after construction. No state transitions.

## Relationships

```text
LoggerComponent --provides--> ILogger
LoggerComponent --contains--> LogLevel (threshold)
LoggerComponent --contains--> Mutex<Writer> (output sink)
```

Other components declare `receptacles: { logger: ILogger }` and bind
to LoggerComponent at wiring time via `connect_receptacle_raw`.
